//! VisionFive 2 v1.3B GMAC1 platform glue.
//!
//! Both the current Linux DT binding and StarFive's legacy U-Boot 2021.10
//! tree are accepted. RMII (the v1.2A GMAC1 wiring) is deliberately rejected.

use alloc::vec::Vec;
use core::mem::size_of;

use axdriver_net::dwmac_jh7110::{
    DwmacJh7110, Jh7110AxiConfig, Jh7110Config, Jh7110DmaConfig, Jh7110Hal, Jh7110PhyConfig,
    jh7110_axi_blen_mask, jh7110_valid_pbl,
};
use fdt_parser::{Fdt, Node, Status};
use log::{info, warn};

use crate::{AxDeviceEnum, drivers::DriverProbe};

const CACHE_LINE_SIZE: usize = 64;
const CCACHE_FLUSH64_OFFSET: usize = 0x200;
const CLOCK_ENABLE: u32 = 1 << 31;
const CLOCK_REGISTER_STRIDE: usize = 4;
// Raw CRG registers that have writable gate bits. Mainline names TX clock 106
// (the inverter), but that node has no gate; its gated parent is GMAC1_TX 105.
const GMAC1_CLOCKS: [usize; 5] = [97, 98, 102, 105, 107];
const SYS_RESET_ASSERT_OFFSET: usize = 0x2f8;
const SYS_RESET_STATUS_OFFSET: usize = 0x308;
const GMAC1_AXI_RESET: usize = 66;
const GMAC1_AHB_RESET: usize = 67;
const RESET_TIMEOUT: usize = 100_000;
const GMAC_REGISTER_WINDOW_MIN: usize = 0x1164;

pub struct GmacHalImpl;

impl GmacHalImpl {
    fn cache_flush(virtual_address: usize, size: usize) {
        if size == 0 {
            return;
        }
        let start = axhal::mem::virt_to_phys(virtual_address.into()).as_usize();
        let Some(end) = start.checked_add(size) else {
            return;
        };
        let mut line = start & !(CACHE_LINE_SIZE - 1);
        let flush = axhal::mem::phys_to_virt(
            (axconfig::devices::CCACHE_PADDR as usize + CCACHE_FLUSH64_OFFSET).into(),
        )
        .as_usize() as *mut u64;
        Self::dma_barrier();
        while line < end {
            unsafe { flush.write_volatile(line as u64) };
            line += CACHE_LINE_SIZE;
        }
        Self::dma_barrier();
    }
}

fn read_register(base: usize, offset: usize) -> u32 {
    unsafe { ((base + offset) as *const u32).read_volatile() }
}

fn write_register(base: usize, offset: usize, value: u32) {
    unsafe { ((base + offset) as *mut u32).write_volatile(value) }
}

fn enable_gmac1_clock_reset() -> bool {
    let base = axhal::mem::phys_to_virt(axconfig::devices::SYSCRG_PADDR.into()).as_usize();

    // U-Boot may disable clocks used for TFTP before jumping to MOSS. Enable
    // the hardware gates directly because MOSS has no general clock provider.
    for clock in GMAC1_CLOCKS {
        let offset = clock * CLOCK_REGISTER_STRIDE;
        write_register(base, offset, read_register(base, offset) | CLOCK_ENABLE);
    }
    GmacHalImpl::dma_barrier();

    let reset_word = GMAC1_AXI_RESET / 32;
    let reset_mask = (1 << (GMAC1_AXI_RESET % 32)) | (1 << (GMAC1_AHB_RESET % 32));
    let assert_offset = SYS_RESET_ASSERT_OFFSET + reset_word * size_of::<u32>();
    let status_offset = SYS_RESET_STATUS_OFFSET + reset_word * size_of::<u32>();
    write_register(
        base,
        assert_offset,
        read_register(base, assert_offset) & !reset_mask,
    );
    GmacHalImpl::dma_barrier();

    for _ in 0..RESET_TIMEOUT {
        if read_register(base, status_offset) & reset_mask == reset_mask {
            return true;
        }
        core::hint::spin_loop();
    }
    warn!("JH7110 GMAC1 reset deassertion timed out");
    false
}

impl Jh7110Hal for GmacHalImpl {
    fn virt_to_phys(virtual_address: usize) -> usize {
        axhal::mem::virt_to_phys(virtual_address.into()).as_usize()
    }

    fn current_ms() -> u64 {
        axhal::time::monotonic_time_nanos() / 1_000_000
    }

    fn dma_clean(virtual_address: usize, size: usize) {
        Self::cache_flush(virtual_address, size);
    }

    fn dma_invalidate(virtual_address: usize, size: usize) {
        // JH7110 exposes one L2 clean-and-invalidate command for both DMA
        // ownership directions.
        Self::cache_flush(virtual_address, size);
    }

    fn dma_barrier() {
        unsafe { core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags)) };
    }
}

pub type Jh7110GmacDevice = DwmacJh7110<GmacHalImpl>;

fn enabled(node: &Node<'_>) -> bool {
    node.status() != Some(Status::Disabled)
}

fn compatible(node: &Node<'_>, wanted: &str) -> bool {
    node.compatibles().any(|actual| actual == wanted)
}

fn first_reg(node: &Node<'_>) -> Option<(usize, usize)> {
    node.reg()?
        .next()
        .map(|reg| (reg.address as usize, reg.size.unwrap_or(size_of::<u32>())))
}

fn first_interrupt(node: &Node<'_>) -> Option<usize> {
    node.interrupts()?.next()?.next().map(|irq| irq as usize)
}

fn gmac1_node<'a>(fdt: &'a Fdt<'a>) -> Option<Node<'a>> {
    let path = fdt.find_aliase("ethernet1")?;
    fdt.find_nodes(path).find(|node| {
        enabled(node)
            && (compatible(node, "starfive,jh7110-dwmac")
                || compatible(node, "starfive,jh7110-eqos-5.20"))
    })
}

/// Locate the legacy PHY only inside the aliased GMAC1 subtree.
///
/// StarFive's 2021.10 DT omits `phy-handle`, and both MAC instances can have
/// an `ethernet-phy@1`. A global name lookup can therefore bind GMAC1 to the
/// first PHY belonging to an unrelated controller.
fn legacy_phy_node<'a>(fdt: &Fdt<'a>, gmac: &Node<'a>) -> Option<Node<'a>> {
    let gmac_address = first_reg(gmac)?.0;
    let mut inside_gmac = false;
    for candidate in fdt.all_nodes() {
        if !inside_gmac {
            inside_gmac = candidate.level == gmac.level
                && candidate.name() == gmac.name()
                && first_reg(&candidate).is_some_and(|(address, _)| address == gmac_address);
            continue;
        }
        if candidate.level <= gmac.level {
            break;
        }
        if candidate.name().split('@').next() == Some("ethernet-phy") {
            return Some(candidate);
        }
    }
    None
}

fn mac_address(node: &Node<'_>) -> Option<[u8; 6]> {
    node.find_property("local-mac-address")
        .or_else(|| node.find_property("mac-address"))
        .and_then(|property| {
            property
                .raw_value()
                .get(..6)
                .and_then(|mac| mac.try_into().ok())
        })
}

fn bounded_u8(node: &Node<'_>, name: &str, max: u32) -> Option<u8> {
    node.find_property(name).map(|property| {
        let value = property.u32();
        assert!(
            value <= max,
            "JH7110 GMAC property {name}={value} exceeds {max}"
        );
        value as u8
    })
}

fn pbl(node: &Node<'_>, name: &str) -> Option<u8> {
    node.find_property(name).map(|property| {
        let value = property.u32();
        assert!(
            jh7110_valid_pbl(value),
            "JH7110 GMAC property {name}={value} is not one of 1,2,4,8,16,32"
        );
        value as u8
    })
}

fn dma_config(fdt: &Fdt<'_>, node: &Node<'_>) -> Jh7110DmaConfig {
    let mut config = Jh7110DmaConfig::linux_default();
    config.tx_pbl = pbl(node, "snps,txpbl").unwrap_or(config.tx_pbl);
    config.rx_pbl = pbl(node, "snps,rxpbl").unwrap_or(config.rx_pbl);
    config.pbl_x8 = node.find_property("snps,no-pbl-x8").is_none();
    config.fixed_burst = node.find_property("snps,fixed-burst").is_some();
    config.force_threshold = node.find_property("snps,force_thresh_dma_mode").is_some();
    config.tx_fifo_depth = node
        .find_property("tx-fifo-depth")
        .map_or(0, |property| property.u32() as usize);
    config.rx_fifo_depth = node
        .find_property("rx-fifo-depth")
        .map_or(0, |property| property.u32() as usize);

    if let Some(axi) = node
        .find_property("snps,axi-config")
        .and_then(|property| fdt.get_node_by_phandle(property.u32().into()))
    {
        let burst_register_bits = axi.find_property("snps,blen").map_or(0, |bursts| {
            let values = bursts.u32_list().collect::<Vec<_>>();
            jh7110_axi_blen_mask(&values).expect(
                "JH7110 snps,blen must contain seven cells, each 0 or a power of two from 4 to 256",
            )
        });
        config.axi = Jh7110AxiConfig {
            write_outstanding_limit: bounded_u8(&axi, "snps,wr_osr_lmt", 0xf).unwrap_or(1),
            read_outstanding_limit: bounded_u8(&axi, "snps,rd_osr_lmt", 0xf).unwrap_or(1),
            burst_register_bits,
        };
    }
    config
}

fn legacy_boolean(node: &Node<'_>, name: &str) -> bool {
    node.find_property(name).is_some_and(|property| {
        let raw = property.raw_value();
        match raw {
            [] => true,
            [a, b, c, d] => u32::from_be_bytes([*a, *b, *c, *d]) != 0,
            _ => panic!("legacy JH7110 PHY property {name} must be empty or one cell"),
        }
    })
}

fn phy_config(phy: &Node<'_>, legacy: bool) -> Jh7110PhyConfig {
    if legacy {
        Jh7110PhyConfig {
            adjust_tx_clock: phy.find_property("tx_inverted_10").is_some()
                || phy.find_property("tx_inverted_100").is_some()
                || phy.find_property("tx_inverted_1000").is_some(),
            tx_clock_inverted_10: legacy_boolean(phy, "tx_inverted_10"),
            tx_clock_inverted_100: legacy_boolean(phy, "tx_inverted_100"),
            tx_clock_inverted_1000: legacy_boolean(phy, "tx_inverted_1000"),
        }
    } else {
        Jh7110PhyConfig {
            adjust_tx_clock: phy.find_property("motorcomm,tx-clk-adj-enabled").is_some(),
            tx_clock_inverted_10: phy.find_property("motorcomm,tx-clk-10-inverted").is_some(),
            tx_clock_inverted_100: phy.find_property("motorcomm,tx-clk-100-inverted").is_some(),
            tx_clock_inverted_1000: phy
                .find_property("motorcomm,tx-clk-1000-inverted")
                .is_some(),
        }
    }
}

fn configure_rgmii_syscon(fdt: &Fdt<'_>, node: &Node<'_>, legacy: bool) {
    let parsed = node.find_property("starfive,syscon").and_then(|property| {
        let mut cells = property.u32_list();
        let phandle = cells.next()?;
        let offset = cells.next()? as usize;
        let shift = cells.next()?;
        let syscon = fdt.get_node_by_phandle(phandle.into())?;
        let (paddr, _) = first_reg(&syscon)?;
        Some((paddr, offset, shift))
    });
    let (paddr, offset, shift) = match parsed {
        Some(value) => value,
        None if legacy => (axconfig::devices::SYSCON_PADDR as usize, 0x90, 2),
        None => panic!("mainline JH7110 GMAC1 DT requires starfive,syscon"),
    };
    assert!(shift <= 29, "invalid JH7110 GMAC1 syscon shift");
    let base = axhal::mem::phys_to_virt(paddr.into()).as_usize();
    let current = read_register(base, offset);
    write_register(base, offset, (current & !(0x7 << shift)) | (1 << shift));
    GmacHalImpl::dma_barrier();
}

fn validate_mainline_clock_reset_contract<'a>(node: &'a Node<'a>) {
    let clocks = node.clocks().collect::<Vec<_>>();
    for (required, id) in [
        ("stmmaceth", 98),
        ("pclk", 97),
        ("ptp_ref", 102),
        ("tx", 106),
        ("gtx", 107),
    ] {
        assert!(
            clocks
                .iter()
                .any(|clock| clock.name == Some(required) && clock.select == id),
            "JH7110 GMAC1 clock {required} must select SYSCRG ID {id}"
        );
    }

    let reset_names = node
        .find_property("reset-names")
        .expect("mainline JH7110 GMAC1 DT has no reset-names")
        .str_list()
        .collect::<Vec<_>>();
    let reset_cells = node
        .find_property("resets")
        .expect("mainline JH7110 GMAC1 DT has no resets")
        .u32_list()
        .collect::<Vec<_>>();
    assert_eq!(
        reset_names,
        ["stmmaceth", "ahb"],
        "JH7110 GMAC1 reset-names must preserve binding order"
    );
    assert!(
        reset_cells.len() == 4 && reset_cells[1] == 66 && reset_cells[3] == 67,
        "JH7110 GMAC1 resets must select SYSCRG AXI 66 then AHB 67"
    );
    assert_eq!(
        reset_cells[0], reset_cells[2],
        "JH7110 GMAC1 resets must use one SYSCRG provider"
    );
    if let Some(first) = clocks.first() {
        for clock in &clocks[1..] {
            assert_eq!(
                clock.node.phandle(),
                first.node.phandle(),
                "JH7110 GMAC1 clocks must use one SYSCRG provider"
            );
        }
    }
}

pub struct Jh7110GmacDriver;

impl DriverProbe for Jh7110GmacDriver {
    fn probe_global() -> Option<AxDeviceEnum> {
        let fdt = axhal::dtb::get_fdt().expect("VisionFive 2 GMAC1 requires a DTB");
        let node = gmac1_node(fdt)?;
        let legacy = compatible(&node, "starfive,jh7110-eqos-5.20");
        let phy_mode = node
            .find_property("phy-mode")
            .map(|property| property.str())
            .expect("JH7110 GMAC1 DT must state phy-mode explicitly");
        assert_eq!(
            phy_mode, "rgmii-id",
            "MOSS supports VF2 v1.3B GMAC1 RGMII-ID only; RMII/v1.2A is unsupported"
        );
        if !legacy {
            validate_mainline_clock_reset_contract(&node);
        }

        let (paddr, size) = first_reg(&node).expect("enabled JH7110 GMAC1 node has no reg");
        assert!(
            size >= GMAC_REGISTER_WINDOW_MIN,
            "JH7110 GMAC1 register window is too small"
        );
        let irq = first_interrupt(&node).or_else(|| {
            if legacy {
                Some(axconfig::devices::GMAC1_IRQ as usize)
            } else {
                None
            }
        });
        let phy_node = node
            .find_property("phy-handle")
            .and_then(|property| fdt.get_node_by_phandle(property.u32().into()))
            .or_else(|| legacy.then(|| legacy_phy_node(fdt, &node)).flatten())
            .expect("JH7110 GMAC1 requires a PHY node in its own MDIO subtree");
        let register_address = phy_node
            .reg()
            .and_then(|mut registers| registers.next().map(|reg| reg.address));
        let phy_address = match register_address {
            Some(address) => u8::try_from(address)
                .ok()
                .filter(|address| *address < 32)
                .expect("JH7110 GMAC1 PHY reg is outside the Clause 22 address range"),
            // The vendor 2021.10 node is named ethernet-phy@1 but has no reg;
            // the VF2 v1.3B YT8531 is physically strapped to MDIO address 0.
            None if legacy => 0,
            None => panic!("mainline JH7110 GMAC1 PHY requires reg"),
        };
        let phy = phy_config(&phy_node, legacy);
        let dma = dma_config(fdt, &node);

        if !enable_gmac1_clock_reset() {
            return None;
        }
        configure_rgmii_syscon(fdt, &node, legacy);

        let base = axhal::mem::phys_to_virt(paddr.into()).as_usize();
        let mut device = unsafe {
            Jh7110GmacDevice::try_new(
                base,
                Jh7110Config {
                    mac_address: mac_address(&node),
                    phy_address,
                    irq,
                    dma,
                    phy,
                },
            )
        }
        .expect("failed to initialize DT-discovered JH7110 GMAC1");

        if let Some(irq) = irq {
            if axhal::irq::register(irq, axdriver_net::dwmac_jh7110::handle_interrupt) {
                device.register_interrupt_handler();
            } else {
                warn!("JH7110 GMAC1 IRQ {irq} registration failed; using polling mode");
            }
        } else {
            warn!("JH7110 GMAC1 has no DT IRQ; using polling mode");
        }
        info!(
            "JH7110 GMAC1 from {} DTB: paddr={paddr:#x}, size={size:#x}, irq={irq:?}, \
             phy_mode={phy_mode}, phy_address={phy_address}, dma={dma:?}, phy={phy:?}",
            if legacy { "legacy U-Boot" } else { "mainline" },
        );
        Some(AxDeviceEnum::from_net(device))
    }
}
