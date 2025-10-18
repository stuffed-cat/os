//! Prototype `shutdown` command that serializes a power-off request.

use userland::{print_requests, SyscallRequest};

fn main() {
    print_requests([SyscallRequest::Shutdown]);
}
