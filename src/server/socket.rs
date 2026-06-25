use std::io;
use std::mem;
use std::os::unix::io::RawFd;

/// Creates a non-blocking IPv4 TCP listening socket bound to `host:port`.
pub fn create_listener(host: &str, port: u16) -> io::Result<RawFd> {
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }

        let optval: libc::c_int = 1;
        let ret = libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &optval as *const _ as *const libc::c_void,
            mem::size_of::<libc::c_int>() as u32,
        );
        if ret < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }

        let ip = parse_ipv4(host).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid host '{}'", host),
            )
        })?;
        let addr = libc::sockaddr_in {
            sin_family: libc::AF_INET as libc::sa_family_t,
            sin_port: port.to_be(),
            sin_addr: libc::in_addr { s_addr: ip },
            sin_zero: [0; 8],
        };

        let ret = libc::bind(
            fd,
            &addr as *const libc::sockaddr_in as *const libc::sockaddr,
            mem::size_of::<libc::sockaddr_in>() as u32,
        );
        if ret < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }

        let ret = libc::listen(fd, 1024);
        if ret < 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }

        Ok(fd)
    }
}

/// Accepts a single pending connection on `listen_fd`, if any.
pub fn accept_conn(listen_fd: RawFd) -> io::Result<Option<(RawFd, String)>> {
    unsafe {
        let mut addr: libc::sockaddr_in = mem::zeroed();
        let mut len = mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        let client = libc::accept4(
            listen_fd,
            &mut addr as *mut libc::sockaddr_in as *mut libc::sockaddr,
            &mut len,
            libc::SOCK_NONBLOCK,
        );
        if client < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EAGAIN)
                || err.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Ok(None);
            }
            return Err(err);
        }
        let octets = addr.sin_addr.s_addr.to_le_bytes();
        let ip = format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]);
        Ok(Some((client, ip)))
    }
}

/// Parses a dotted-quad IPv4 address 
fn parse_ipv4(host: &str) -> Option<u32> {
    if host == "*" || host == "0.0.0.0" {
        return Some(0);
    }
    if host == "localhost" {
        return Some(u32::from_le_bytes([127, 0, 0, 1]));
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        bytes[i] = p.parse().ok()?;
    }
    Some(u32::from_le_bytes(bytes))
}
