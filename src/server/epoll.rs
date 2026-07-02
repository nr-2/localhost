use std::io;
use std::os::unix::io::RawFd;

pub const EPOLLIN: u32 = libc::EPOLLIN as u32;
pub const EPOLLOUT: u32 = libc::EPOLLOUT as u32;
pub const EPOLLERR: u32 = libc::EPOLLERR as u32;
pub const EPOLLHUP: u32 = libc::EPOLLHUP as u32;
pub const EPOLLRDHUP: u32 = libc::EPOLLRDHUP as u32;

pub struct Epoll {
    fd: RawFd,
}

impl Epoll {
    pub fn new() -> io::Result<Self> {
        let fd = unsafe { libc::epoll_create1(0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Epoll { fd })
    }

    pub fn add(&self, fd: RawFd, events: u32) -> io::Result<()> {
        let mut ev = libc::epoll_event {
            events,
            u64: fd as u64,
        };
        let ret = unsafe { libc::epoll_ctl(self.fd, libc::EPOLL_CTL_ADD, fd, &mut ev) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn modify(&self, fd: RawFd, events: u32) -> io::Result<()> {
        let mut ev = libc::epoll_event {
            events,
            u64: fd as u64,
        };
        let ret = unsafe { libc::epoll_ctl(self.fd, libc::EPOLL_CTL_MOD, fd, &mut ev) };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// Removes `fd` from the interest list. Errors (e.g. the fd was already
    pub fn remove(&self, fd: RawFd) {
        unsafe {
            libc::epoll_ctl(self.fd, libc::EPOLL_CTL_DEL, fd, std::ptr::null_mut());
        }
    }

    pub fn wait(&self, events: &mut [libc::epoll_event], timeout_ms: i32) -> io::Result<usize> {
        let ret = unsafe {
            libc::epoll_wait(
                self.fd,
                events.as_mut_ptr(),
                events.len() as i32,
                timeout_ms,
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            return Err(err);
        }
        Ok(ret as usize)
    }
}

impl Drop for Epoll {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}
