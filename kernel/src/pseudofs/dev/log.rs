use axerrno::LinuxResult;

pub fn bind_dev_log() -> LinuxResult<()> {
    warn!("/dev/log server is disabled because network support is removed");
    Ok(())
}
