//! Prototype `help` command printing the list of available shell commands.

const HELP_ENTRIES: &[(&str, &str)] = &[
    ("help", "Show this help message"),
    ("history", "Display previously executed commands"),
    ("sh", "Launch a nested shell session"),
    ("ls", "List entries from the mounted filesystem"),
    ("pwd", "Print the current working directory"),
    ("cd", "Change the current working directory"),
    ("cat", "Display file contents"),
    ("echo", "Print arguments back to the console"),
    (
        "touch",
        "Create an empty file (overlay-backed; shell wiring WIP)",
    ),
    (
        "mkdir",
        "Create a directory (overlay-backed; shell wiring WIP)",
    ),
    (
        "rmdir",
        "Remove a directory (overlay-backed; shell wiring WIP)",
    ),
    ("rm", "Remove a file (overlay-backed; shell wiring WIP)"),
    ("cp", "Copy a file"),
    (
        "mv",
        "Move or rename a file (overlay-backed; shell wiring WIP)",
    ),
    ("reboot", "Reboot the system"),
    ("shutdown", "Power off the system"),
];

fn main() {
    println!("Bare shell commands:");
    for (command, description) in HELP_ENTRIES {
        println!("  {command:<8} {description}");
    }
}
