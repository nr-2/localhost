use std::ffi::CString;
use std::io;
use std::os::unix::io::RawFd;

use super::io_util::{close_fd, set_nonblocking};

/// State for a forked CGI child process and the pipes connected to it.
pub struct CgiProcess {
    pub pid: libc::pid_t,
    /// Write end of the pipe feeding the CGI's stdin (request body).
    pub stdin_fd: Option<RawFd>,
    /// Read end of the pipe collecting the CGI's stdout.
    pub stdout_fd: Option<RawFd>,
    pub input: Vec<u8>,
    pub input_pos: usize,
    pub output: Vec<u8>,
    reaped: bool,
}

impl CgiProcess {
    /// Forks and execs `interpreter script_path` with `cwd` as its working
    /// directory and `env` as its environment. `body` is the request body
    /// that will be streamed to the CGI's stdin.
    pub fn spawn(
        interpreter: &str,
        script_path: &str,
        cwd: &str,
        env: &[(String, String)],
        body: Vec<u8>,
    ) -> io::Result<CgiProcess> {
        let mut stdin_pipe = [0i32; 2];
        let mut stdout_pipe = [0i32; 2];

        unsafe {
            if libc::pipe(stdin_pipe.as_mut_ptr()) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::pipe(stdout_pipe.as_mut_ptr()) < 0 {
                let err = io::Error::last_os_error();
                close_fd(stdin_pipe[0]);
                close_fd(stdin_pipe[1]);
                return Err(err);
            }
        }

        let interpreter_c = CString::new(interpreter)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in interpreter"))?;
        let script_c = CString::new(script_path)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in script path"))?;
        let cwd_c = CString::new(cwd)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "nul in cwd"))?;

        let argv: [*const libc::c_char; 3] =
            [interpreter_c.as_ptr(), script_c.as_ptr(), std::ptr::null()];

        let env_cstrings: Vec<CString> = env
            .iter()
            .filter_map(|(k, v)| CString::new(format!("{}={}", k, v)).ok())
            .collect();
        let mut envp: Vec<*const libc::c_char> = env_cstrings.iter().map(|c| c.as_ptr()).collect();
        envp.push(std::ptr::null());

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            let err = io::Error::last_os_error();
            close_fd(stdin_pipe[0]);
            close_fd(stdin_pipe[1]);
            close_fd(stdout_pipe[0]);
            close_fd(stdout_pipe[1]);
            return Err(err);
        }

        if pid == 0 {
            // Child: wire up stdio and exec. Avoid any allocation here.
            unsafe {
                libc::dup2(stdin_pipe[0], 0);
                libc::dup2(stdout_pipe[1], 1);
                libc::close(stdin_pipe[0]);
                libc::close(stdin_pipe[1]);
                libc::close(stdout_pipe[0]);
                libc::close(stdout_pipe[1]);
                libc::chdir(cwd_c.as_ptr());
                libc::execve(interpreter_c.as_ptr(), argv.as_ptr(), envp.as_ptr());
                // execve only returns on error.
                libc::_exit(127);
            }
        }

        // Parent
        close_fd(stdin_pipe[0]);
        close_fd(stdout_pipe[1]);
        set_nonblocking(stdin_pipe[1])?;
        set_nonblocking(stdout_pipe[0])?;

        let stdin_fd = if body.is_empty() {
            close_fd(stdin_pipe[1]);
            None
        } else {
            Some(stdin_pipe[1])
        };

        Ok(CgiProcess {
            pid,
            stdin_fd,
            stdout_fd: Some(stdout_pipe[0]),
            input: body,
            input_pos: 0,
            output: Vec::new(),
            reaped: false,
        })
    }

    /// Non-blocking reap of the child process. Returns the wait status once
    /// the child has exited.
    pub fn try_reap(&mut self) -> Option<i32> {
        if self.reaped {
            return None;
        }
        let mut status: i32 = 0;
        let ret = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
        if ret == self.pid {
            self.reaped = true;
            Some(status)
        } else {
            None
        }
    }

    /// Forcefully terminates the child (used on timeout or connection close).
    pub fn kill(&mut self) {
        if !self.reaped {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
            }

            let mut status = 0;
            unsafe {
                libc::waitpid(self.pid, &mut status, libc::WNOHANG);
            }
            self.reaped = true;
        }
    }
}

impl Drop for CgiProcess {
    fn drop(&mut self) {
        if let Some(fd) = self.stdin_fd.take() {
            close_fd(fd);
        }
        if let Some(fd) = self.stdout_fd.take() {
            close_fd(fd);
        }
        if !self.reaped {
            self.kill();
        }
    }
}
