//! Early, allocation-free board resource discovery from the firmware DTB.

use core::ptr::NonNull;

use fdt_parser::{Fdt, Node, Phandle, Status};
use lazyinit::LazyInit;

pub(crate) const LIOINTC_IRQ_BASE: usize = 16;
pub(crate) const LIOINTC_IRQS_PER_CONTROLLER: usize = 32;
pub(crate) const MAX_LIOINTC: usize = 2;
pub(crate) const MAX_CPUS: usize = 2;
pub(crate) const UNCACHED_DMW_BASE: usize = 0x8000_0000_0000_0000;
const PHYS_ADDR_MASK: usize = (1usize << 48) - 1;

#[derive(Clone, Copy, Debug)]
pub(crate) struct UartResource {
    pub paddr: usize,
    pub reg_shift: usize,
    pub reg_io_width: usize,
    pub clock_hz: Option<u32>,
    pub irq: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct LioIntcResource {
    pub regs_paddr: usize,
    pub regs_size: usize,
    pub isr_paddr: usize,
    pub isr_size: usize,
    pub parent_irq: usize,
    pub parent_index: usize,
    pub irq_base: usize,
    pub phandle: Option<Phandle>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct SysconAction {
    pub paddr: usize,
    pub offset: usize,
    pub mask: u32,
    pub value: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Resources {
    pub cpu_hardware_ids: [Option<usize>; MAX_CPUS],
    pub uart: UartResource,
    pub rtc_paddr: Option<usize>,
    pub liointc: [Option<LioIntcResource>; MAX_LIOINTC],
    pub poweroff: Option<SysconAction>,
    pub reboot: Option<SysconAction>,
}

static RESOURCES: LazyInit<Resources> = LazyInit::new();

#[inline]
pub(crate) const fn to_phys(addr: usize) -> usize {
    addr & PHYS_ADDR_MASK
}

#[inline]
pub(crate) const fn uncached_addr(paddr: usize) -> usize {
    UNCACHED_DMW_BASE | to_phys(paddr)
}

pub(crate) fn resources() -> &'static Resources {
    &RESOURCES
}

pub(crate) fn init(dtb_paddr: usize) {
    if RESOURCES.get().is_some() {
        return;
    }
    let dtb_paddr = to_phys(dtb_paddr);
    assert_ne!(dtb_paddr, 0, "LS2K1000 requires a firmware-provided DTB");
    let ptr = NonNull::new(uncached_addr(dtb_paddr) as *mut u8).expect("null DTB address");
    let fdt = Fdt::from_ptr(ptr).expect("invalid LS2K1000 firmware DTB");
    RESOURCES.init_once(parse_resources(&fdt));
}

fn enabled(node: &Node<'_>) -> bool {
    node.status() != Some(Status::Disabled)
}

fn compatible(node: &Node<'_>, choices: &[&str]) -> bool {
    node.compatibles()
        .any(|actual| choices.iter().any(|choice| actual == *choice))
}

fn first_reg(node: &Node<'_>) -> Option<(usize, usize)> {
    node.reg()?.next().map(|reg| {
        (
            reg.address as usize,
            reg.size.unwrap_or(core::mem::size_of::<u32>()),
        )
    })
}

fn first_interrupt(node: &Node<'_>) -> Option<usize> {
    node.interrupts()?
        .next()?
        .next()
        .map(|value| value as usize)
}

fn parse_liointc(fdt: &Fdt<'_>) -> [Option<LioIntcResource>; MAX_LIOINTC] {
    let mut result = [None; MAX_LIOINTC];
    let mut count = 0;
    for node in fdt.find_compatible(&[
        "loongson,2k1000-icu",
        "loongson,ls2k1000-icu",
        "loongson,liointc",
    ]) {
        if !enabled(&node) {
            continue;
        }
        assert!(count < MAX_LIOINTC, "too many enabled LIOINTC nodes");
        let mut regs = node.reg().expect("LIOINTC node has no reg property");
        let register = regs.next().expect("LIOINTC register window is missing");
        let isr = regs.next().expect("LIOINTC ISR window is missing");
        let parent_irq = first_interrupt(&node).expect("LIOINTC parent interrupt is missing");
        let parent_index = parent_irq
            .checked_sub(2)
            .filter(|index| *index < 4)
            .expect("LIOINTC parent interrupt must be a CPU HWI line 2..5");
        result[count] = Some(LioIntcResource {
            regs_paddr: register.address as usize,
            regs_size: register.size.unwrap_or(0x40),
            isr_paddr: isr.address as usize,
            isr_size: isr.size.unwrap_or(0x10),
            parent_irq,
            parent_index,
            irq_base: LIOINTC_IRQ_BASE + count * LIOINTC_IRQS_PER_CONTROLLER,
            phandle: node.phandle(),
        });
        count += 1;
    }
    assert!(count != 0, "no enabled LS2K1000 LIOINTC node in DTB");
    result
}

fn parse_cpu_hardware_ids(fdt: &Fdt<'_>) -> [Option<usize>; MAX_CPUS] {
    let mut ids = [None; MAX_CPUS];
    let mut count = 0;
    for node in fdt.find_nodes("/cpus/cpu").filter(enabled).filter(|node| {
        node.find_property("device_type")
            .is_none_or(|property| property.str() == "cpu")
    }) {
        assert!(count < MAX_CPUS, "more than two enabled CPUs in board DTB");
        let id = node
            .reg()
            .and_then(|mut regs| regs.next())
            .map(|reg| reg.address as usize)
            .expect("enabled CPU node has no reg property");
        assert!(
            !ids[..count].contains(&Some(id)),
            "duplicate CPU hardware ID in board DTB"
        );
        ids[count] = Some(id);
        count += 1;
    }
    assert!(count != 0, "no enabled CPU in board DTB");
    ids[..count].sort_unstable();

    let boot_id = fdt.boot_cpuid_phys() as usize;
    let boot_index = ids[..count]
        .iter()
        .position(|id| *id == Some(boot_id))
        .expect("DTB boot CPU is not enabled");
    ids.swap(0, boot_index);
    ids
}

pub(crate) fn logical_cpu_hardware_id(logical_id: usize) -> usize {
    resources()
        .cpu_hardware_ids
        .get(logical_id)
        .copied()
        .flatten()
        .expect("logical CPU ID is outside the board DT topology")
}

fn translate_irq(
    node: &Node<'_>,
    controllers: &[Option<LioIntcResource>; MAX_LIOINTC],
) -> Option<usize> {
    let parent = node.interrupt_parent()?.node.phandle()?;
    let source = first_interrupt(node)?;
    let controller = controllers
        .iter()
        .flatten()
        .find(|controller| controller.phandle == Some(parent))?;
    (source < LIOINTC_IRQS_PER_CONTROLLER).then_some(controller.irq_base + source)
}

fn parse_uart(fdt: &Fdt<'_>, controllers: &[Option<LioIntcResource>; MAX_LIOINTC]) -> UartResource {
    let stdout = fdt
        .chosen()
        .and_then(|chosen| chosen.stdout())
        .expect("/chosen/stdout-path does not resolve to a UART node");
    let node = stdout.node;
    assert!(enabled(&node), "stdout UART is disabled in DTB");
    assert!(
        compatible(&node, &["ns16550", "ns16550a", "snps,dw-apb-uart"]),
        "stdout UART is not NS16550 compatible"
    );
    let (paddr, _) = first_reg(&node).expect("stdout UART has no reg property");
    let reg_shift = node
        .find_property("reg-shift")
        .map_or(0, |property| property.u32() as usize);
    let reg_io_width = node
        .find_property("reg-io-width")
        .map_or(1, |property| property.u32() as usize);
    assert!(
        matches!(reg_io_width, 1 | 4),
        "unsupported UART reg-io-width"
    );
    UartResource {
        paddr,
        reg_shift,
        reg_io_width,
        clock_hz: node.clock_frequency(),
        irq: translate_irq(&node, controllers),
    }
}

fn parse_rtc(fdt: &Fdt<'_>) -> Option<usize> {
    fdt.find_compatible(&[
        "loongson,ls2k-rtc",
        "loongson,ls2k1000-rtc",
        "loongson,2k1000-rtc",
    ])
    .find(enabled)
    .and_then(|node| first_reg(&node).map(|reg| reg.0))
}

fn parse_syscon_action(fdt: &Fdt<'_>, wanted: &str) -> Option<SysconAction> {
    let compatibles = [wanted];
    let node = fdt.find_compatible(&compatibles).find(enabled)?;
    let phandle = node.find_property("regmap")?.u32().into();
    let syscon = fdt.get_node_by_phandle(phandle)?;
    if !enabled(&syscon) || !compatible(&syscon, &["syscon"]) {
        return None;
    }
    let (paddr, size) = first_reg(&syscon)?;
    let offset = node.find_property("offset").map_or(0, |p| p.u32() as usize);
    if offset.checked_add(core::mem::size_of::<u32>())? > size {
        return None;
    }
    let mask = node
        .find_property("mask")
        .map_or(u32::MAX, |property| property.u32());
    let value = node
        .find_property("value")
        .map_or(mask, |property| property.u32());
    Some(SysconAction {
        paddr,
        offset,
        mask,
        value,
    })
}

fn parse_resources(fdt: &Fdt<'_>) -> Resources {
    let liointc = parse_liointc(fdt);
    Resources {
        cpu_hardware_ids: parse_cpu_hardware_ids(fdt),
        uart: parse_uart(fdt, &liointc),
        rtc_paddr: parse_rtc(fdt),
        liointc,
        poweroff: parse_syscon_action(fdt, "syscon-poweroff"),
        reboot: parse_syscon_action(fdt, "syscon-reboot"),
    }
}
