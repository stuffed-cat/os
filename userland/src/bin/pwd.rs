//! Prototype `pwd` command that prints the working directory from the environment.

use std::env;

fn main() {
    let cwd = env::var("PWD").unwrap_or_else(|_| String::from("/"));
    println!("{cwd}");
}
