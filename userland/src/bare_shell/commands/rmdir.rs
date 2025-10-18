use super::super::{BareShell, FsError, ShellFs, ShellIo, ShellSystem};

pub(super) fn run<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    if args.is_empty() {
        shell.println("usage: rmdir DIRECTORY...");
        return;
    }

    for &arg in args {
        let path = shell.make_absolute_path(Some(arg));
        match shell.fs.remove_dir(&path) {
            Ok(()) => {}
            Err(FsError::Unavailable) => {
                shell.println("rmdir: filesystem unavailable");
                return;
            }
            Err(FsError::NotFound) => {
                shell.print("rmdir: failed to remove '");
                shell.print(&path);
                shell.println("': No such file or directory");
            }
            Err(FsError::NotDirectory) => {
                shell.print("rmdir: failed to remove '");
                shell.print(&path);
                shell.println("': Not a directory");
            }
            Err(FsError::DirectoryNotEmpty) => {
                shell.print("rmdir: failed to remove '");
                shell.print(&path);
                shell.println("': Directory not empty");
            }
            Err(FsError::AlreadyExists) => {
                shell.println("rmdir: filesystem error");
            }
            Err(FsError::PermissionDenied) => shell.println("rmdir: permission denied"),
            Err(FsError::Corrupt) => shell.println("rmdir: filesystem corrupt"),
            Err(FsError::NotFile) => shell.println("rmdir: filesystem error"),
        }
    }
}
