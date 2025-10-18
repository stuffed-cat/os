//! Prototype `touch` command that emits the syscall stream needed to create or
//! truncate a file.

use std::env;

use userland::{print_requests, SyscallRequest, O_CREAT, O_WRONLY};

const TOUCH_MODE: u32 = 0o644;

fn main() {
    let mut args = env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("touch: missing file operand");
        std::process::exit(1);
    };

    if args.next().is_some() {
        eprintln!("touch: multiple operands are not supported yet");
        std::process::exit(1);
    }

    let requests = [
        SyscallRequest::Open {
            path: path.clone(),
            flags: O_WRONLY | O_CREAT,
            mode: TOUCH_MODE,
        },
        SyscallRequest::Close { fd: 3 },
    ];

    print_requests(requests);
}
