//! Policy for selecting the firmware or platform RAM description.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RamSource {
    DeviceTree,
    Platform,
}

/// Visits usable ranges from the DT when present, otherwise the platform
/// fallback. Zero-sized entries do not make a source usable.
pub(crate) fn visit_selected_ram_ranges<D, P, F>(
    device_tree: D,
    platform: P,
    mut visit: F,
) -> RamSource
where
    D: IntoIterator<Item = (usize, usize)>,
    P: IntoIterator<Item = (usize, usize)>,
    F: FnMut((usize, usize)),
{
    let mut device_tree = device_tree
        .into_iter()
        .filter(|(_, size)| *size != 0)
        .peekable();
    if device_tree.peek().is_some() {
        device_tree.for_each(&mut visit);
        RamSource::DeviceTree
    } else {
        platform
            .into_iter()
            .filter(|(_, size)| *size != 0)
            .for_each(visit);
        RamSource::Platform
    }
}

#[cfg(test)]
mod tests {
    use super::{RamSource, visit_selected_ram_ranges};

    #[test]
    fn device_tree_ranges_take_precedence() {
        let mut selected = Vec::new();
        let source = visit_selected_ram_ranges(
            [(0x20_0000, 0xae0_0000)],
            [(0, 0x1000_0000), (0x9000_0000, 0x7000_0000)],
            |range| selected.push(range),
        );

        assert_eq!(source, RamSource::DeviceTree);
        assert_eq!(selected, [(0x20_0000, 0xae0_0000)]);
    }

    #[test]
    fn missing_device_tree_memory_uses_platform_ranges() {
        let mut selected = Vec::new();
        let source = visit_selected_ram_ranges(
            [],
            [(0, 0x1000_0000), (0x9000_0000, 0x7000_0000)],
            |range| selected.push(range),
        );

        assert_eq!(source, RamSource::Platform);
        assert_eq!(selected, [(0, 0x1000_0000), (0x9000_0000, 0x7000_0000)]);
    }

    #[test]
    fn zero_sized_device_tree_entries_do_not_suppress_fallback() {
        let mut selected = Vec::new();
        let source =
            visit_selected_ram_ranges([(0x8000_0000, 0)], [(0x9000_0000, 0x1000)], |range| {
                selected.push(range)
            });

        assert_eq!(source, RamSource::Platform);
        assert_eq!(selected, [(0x9000_0000, 0x1000)]);
    }
}
