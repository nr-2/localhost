mod config;
mod handler;
mod http;
mod server;
mod util;

use std::process::ExitCode;

fn main() -> ExitCode {
    // Writing to a socket whose peer has gone away delivers SIGPIPE by
    // default, which would kill the process. We handle EPIPE as a normal
    // I/O error instead.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_IGN);
    }

    let args: Vec<String> = std::env::args().collect();
    let config_path = if args.len() > 1 {
        args[1].clone()
    } else {
        "conf/default.conf".to_string()
    };

    let config = match config::parse_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("configuration error in '{}': {}", config_path, e);
            return ExitCode::FAILURE;
        }
    };

    let mut server = match server::Server::new(config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to start server: {}", e);
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = server.run() {
        eprintln!("server loop exited with error: {}", e);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
