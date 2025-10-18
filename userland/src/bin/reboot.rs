//! Prototype `reboot` command that serializes a reboot request.

use userland::{print_requests, SyscallRequest};

fn main() {
    print_requests([SyscallRequest::Reboot]);
}
