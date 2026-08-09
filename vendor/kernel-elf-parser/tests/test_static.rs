use kernel_elf_parser::{ELFHeadersBuilder, ELFParser};

fn parse_elf(bytes: &[u8]) -> kernel_elf_parser::ELFHeaders<'_> {
    let builder = ELFHeadersBuilder::new(bytes).unwrap();
    let range = builder.ph_range();
    builder
        .build(&bytes[range.start as usize..range.end as usize])
        .unwrap()
}

#[test]
fn test_elf_parser() {
    // A simple elf file compiled by the x86_64-linux-musl-gcc.
    let elf_bytes = include_bytes!("elf_static");
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
    let headers = parse_elf(&aligned_elf_bytes);

    let interp_base = 0x1000;
    let elf_parser = ELFParser::new(&headers, interp_base).unwrap();
    let base_addr = elf_parser.base();
    assert_eq!(base_addr, 0);

    let segments = elf_parser
        .headers()
        .ph
        .iter()
        .filter(|segment| segment.get_type() == Ok(xmas_elf::program::Type::Load))
        .collect::<Vec<_>>();
    assert_eq!(segments.len(), 4);
    let mut last_start = 0;
    for segment in segments.iter() {
        // start vaddr should be sorted
        assert!(segment.virtual_addr > last_start);
        last_start = segment.virtual_addr;
    }
    assert_eq!(segments[0].virtual_addr, 0x400000);

    test_ustack(&elf_parser);
}

fn test_ustack(elf_parser: &ELFParser) {
    let auxv = elf_parser
        .aux_vector(0x1000, None)
        .unwrap()
        .collect::<Vec<_>>();
    // let phent = auxv.get(&AT_PHENT).unwrap();
    // assert_eq!(*phent, 56);
    auxv.iter().for_each(|entry| {
        if entry.get_type() == kernel_elf_parser::AuxType::PHENT {
            assert_eq!(entry.value(), 56);
        }
    });

    let args: Vec<String> = vec!["arg1".to_string(), "arg2".to_string(), "arg3".to_string()];
    let envs: Vec<String> = vec!["LOG=file".to_string()];

    // The highest address of the user stack.
    let ustack_end = 0x4000_0000;

    let stack_data = kernel_elf_parser::app_stack_region(&args, &envs, &auxv, ustack_end);
    // The first 8 bytes of the stack is the number of arguments.
    assert_eq!(stack_data[0..8], [3, 0, 0, 0, 0, 0, 0, 0]);
}

#[test]
fn program_headers_outside_load_segment_are_rejected() {
    let mut elf = include_bytes!("elf_static").to_vec();
    // Elf64_Phdr::p_filesz of the first PT_LOAD.  Truncate that segment at the
    // ELF header so it no longer covers the program header table at e_phoff.
    elf[64 + 32..64 + 40].copy_from_slice(&64u64.to_le_bytes());

    let headers = parse_elf(&elf);
    let parser = ELFParser::new(&headers, 0).unwrap();
    assert!(parser.phdr().is_err());
    assert!(parser.aux_vector(0x1000, None).is_err());
}
