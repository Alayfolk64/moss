//! Pure pathname/dirfd selection shared by filesystem `*at` syscalls.

/// Selects the directory descriptor that Linux pathname lookup must use.
///
/// Absolute paths are rooted at the process root and must ignore `dirfd`,
/// including an otherwise invalid descriptor.
pub(crate) const fn effective_dirfd(
    dirfd: i32,
    path_is_absolute: bool,
    at_fdcwd: i32,
) -> i32 {
    if path_is_absolute { at_fdcwd } else { dirfd }
}

#[cfg(test)]
mod tests {
    use super::effective_dirfd;

    const AT_FDCWD: i32 = -100;

    #[test]
    fn absolute_path_ignores_invalid_dirfd() {
        assert_eq!(effective_dirfd(-1, true, AT_FDCWD), AT_FDCWD);
    }

    #[test]
    fn relative_path_keeps_caller_dirfd() {
        assert_eq!(effective_dirfd(37, false, AT_FDCWD), 37);
    }
}
