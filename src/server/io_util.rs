use std::io;
use std::os::unix::io::RawFd;

/// Reads from `fd` into `buf`, returning `Ok(0)` on EOF.
pub fn read_fd(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    let ret = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ret as usize)
}

/// Writes `buf` to `fd`, returning the number of bytes written.
pub fn write_fd(fd: RawFd, buf: &[u8]) -> io::Result<usize> {
    let ret = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(ret as usize)
}

/// Sets the `O_NONBLOCK` flag on `fd`.
pub fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL, 0);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        let ret = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Closes `fd`, ignoring errors (the fd may already be closed).
pub fn close_fd(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}

/// Returns `true` if `err` indicates the operation would block on a
pub fn would_block(err: &io::Error) -> bool {
    err.raw_os_error() == Some(libc::EAGAIN)
}
