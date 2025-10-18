//! Prototype `cp` command that emits a syscall sequence for copying files.

use std::{env, fs};

use userland::{print_requests, SyscallRequest, O_CREAT, O_RDONLY, O_TRUNC, O_WRONLY};

const CHUNK_SIZE: usize = 4096;
const DEST_MODE: u32 = 0o644;

fn main() {
    let mut args = env::args().skip(1);
    let Some(src) = args.next() else {
        eprintln!("cp: missing source operand");
        std::process::exit(1);
    };
    let Some(dest) = args.next() else {
        eprintln!("cp: missing destination operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("cp: multiple sources and options are not supported yet");
        std::process::exit(1);
    }

    let mut requests = Vec::new();
    requests.push(SyscallRequest::Open {
        path: src.clone(),
        flags: O_RDONLY,
        mode: 0,
    });
    requests.push(SyscallRequest::Open {
        path: dest.clone(),
        flags: O_WRONLY | O_CREAT | O_TRUNC,
        mode: DEST_MODE,
    });

    match fs::read(&src) {
        Ok(bytes) => {
            for chunk in bytes.chunks(CHUNK_SIZE) {
                requests.push(SyscallRequest::Read {
                    fd: 3,
                    len: chunk.len() as u64,
                });
                requests.push(SyscallRequest::Write {
                    fd: 4,
                    data: chunk.to_vec(),
                });
            }
        }
        Err(err) => {
            eprintln!("cp: warning: preview read of `{}` failed: {}", src, err);
            requests.push(SyscallRequest::Read {
                fd: 3,
                len: CHUNK_SIZE as u64,
            });
            requests.push(SyscallRequest::Write {
                fd: 4,
                data: Vec::new(),
            });
        }
    }

    requests.push(SyscallRequest::Close { fd: 3 });
    requests.push(SyscallRequest::Close { fd: 4 });

    print_requests(requests);
}
