use loongArch64::ipi::{csr_mail_send, send_ipi_single};

use crate::config::plat::{PHYS_BOOT_OFFSET, PHYS_VIRT_OFFSET};

const ACTION_BOOT_CPU: u32 = 1;

pub(crate) fn start_secondary_cpu(cpu_id: usize, stack_top_paddr: usize) {
    let entry =
        crate::boot::_start_secondary as *const () as usize - PHYS_VIRT_OFFSET + PHYS_BOOT_OFFSET;
    let stack = stack_top_paddr | PHYS_BOOT_OFFSET;

    // LS2K1000 mailbox 1 carries the initial stack and mailbox 0 the entry.
    // Publish both before issuing the boot IPI, matching the firmware protocol.
    csr_mail_send(stack as _, cpu_id, 1);
    csr_mail_send(entry as _, cpu_id, 0);
    send_ipi_single(cpu_id, ACTION_BOOT_CPU);
}
