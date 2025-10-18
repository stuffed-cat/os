use super::super::{BareShell, FsError, ShellFs, ShellIo, ShellSystem};

pub(super) fn run<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    if args.is_empty() {
        shell.println("usage: mkdir DIRECTORY...");
        return;
    }

    for &arg in args {
        let path = shell.make_absolute_path(Some(arg));
        match shell.fs.create_dir(&path, 0o755) {
            Ok(()) => {}
            Err(FsError::AlreadyExists) => {
                shell.print("mkdir: cannot create directory '");
                shell.print(&path);
                shell.println("': File exists");
            }
            Err(FsError::Unavailable) => {
                shell.println("mkdir: filesystem unavailable");
                return;
            }
            Err(FsError::NotFound) => {
                shell.print("mkdir: parent directory not found: ");
                shell.println(&path);
            }
            Err(FsError::NotDirectory) => {
                shell.print("mkdir: parent is not a directory: ");
                shell.println(&path);
            }
            Err(FsError::PermissionDenied) => shell.println("mkdir: permission denied"),
            Err(FsError::Corrupt) => shell.println("mkdir: filesystem corrupt"),
            Err(FsError::NotFile) | Err(FsError::DirectoryNotEmpty) => {
                shell.println("mkdir: filesystem error");
            }
        }
    }
}
