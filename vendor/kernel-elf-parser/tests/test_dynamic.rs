use kernel_elf_parser::ELFHeadersBuilder;

#[test]
fn test_elf_parser() {
    let elf_bytes = include_bytes!("ld-linux-x86-64.so.2");
    // Ensure the alignment of the byte array
    let mut aligned_elf_bytes = unsafe {
        let ptr = elf_bytes.as_ptr() as *mut u8;
        std::slice::from_raw_parts_mut(ptr, elf_bytes.len())
    }
    .to_vec();
    if aligned_elf_bytes.len() % 16 != 0 {
        let padding = vec![0u8; 16 - aligned_elf_bytes.len() % 16];
        aligned_elf_bytes.extend(padding);
    }
    let builder = ELFHeadersBuilder::new(&aligned_elf_bytes).unwrap();
    let range = builder.ph_range();
    let headers = builder
        .build(&aligned_elf_bytes[range.start as usize..range.end as usize])
        .unwrap();
    let interp_base = 0x1000;
    let elf_parser = kernel_elf_parser::ELFParser::new(&headers, interp_base).unwrap();
    let base_addr = elf_parser.base();
    assert_eq!(base_addr, interp_base);

    let segments = elf_parser
        .headers()
        .ph
        .iter()
        .filter(|segment| segment.get_type() == Ok(xmas_elf::program::Type::Load))
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 4);
    for segment in segments.iter() {
        println!(
            "{:?} {:?}",
            segment.virtual_addr as usize + elf_parser.base(),
            segment.flags
        );
    }
    assert_eq!(
        segments[0].virtual_addr as usize + elf_parser.base(),
        0x1000
    );
}
