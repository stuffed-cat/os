//! Prototype `echo` command.

use std::env;

fn main() {
    let output = env::args().skip(1).collect::<Vec<_>>().join(" ");
    println!("{output}");
}
