const MAX_UBOOT_GO_ARG_LEN: usize = 32;

fn parse_hex_bytes(bytes: &[u8]) -> Option<usize> {
    let bytes = bytes
        .strip_prefix(b"0x")
        .or_else(|| bytes.strip_prefix(b"0X"))
        .unwrap_or(bytes);
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0usize, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        } as usize;
        value.checked_mul(16)?.checked_add(digit)
    })
}

/// Parse one hexadecimal argument from U-Boot's command buffer.
///
/// U-Boot does not promise that strings referenced by `argv` are naturally
/// aligned. Volatile byte loads keep LLVM from combining adjacent reads into
/// an unaligned halfword load, which raises ALE on LS2K1000LA when UAL is off.
pub(crate) unsafe fn parse_hex_c_str(ptr: *const u8) -> Option<usize> {
    if ptr.is_null() {
        return None;
    }

    let mut bytes = [0u8; MAX_UBOOT_GO_ARG_LEN];
    let mut len = 0;
    while len < bytes.len() {
        let byte = unsafe { ptr.add(len).read_volatile() };
        if byte == 0 {
            return parse_hex_bytes(&bytes[..len]);
        }
        bytes[len] = byte;
        len += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_hex_c_str;

    #[test]
    fn parses_prefixed_and_plain_addresses() {
        let prefixed = b"0x900000000a000000\0";
        let plain = b"98000040\0";
        assert_eq!(
            unsafe { parse_hex_c_str(prefixed.as_ptr()) },
            Some(0x9000_0000_0a00_0000)
        );
        assert_eq!(
            unsafe { parse_hex_c_str(plain.as_ptr()) },
            Some(0x9800_0040)
        );
    }

    #[test]
    fn parses_an_unaligned_uboot_argument() {
        let command_buffer = b"x0x900000000a000000\0";
        let unaligned = unsafe { command_buffer.as_ptr().add(1) };
        assert_eq!(unaligned as usize & 1, 1);
        assert_eq!(
            unsafe { parse_hex_c_str(unaligned) },
            Some(0x9000_0000_0a00_0000)
        );
    }

    #[test]
    fn rejects_invalid_or_unterminated_arguments() {
        assert_eq!(unsafe { parse_hex_c_str(b"0xnot-hex\0".as_ptr()) }, None);
        assert_eq!(unsafe { parse_hex_c_str([b'a'; 32].as_ptr()) }, None);
    }
}
