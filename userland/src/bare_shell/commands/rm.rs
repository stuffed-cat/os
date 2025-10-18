use super::super::{BareShell, FsError, ShellFs, ShellIo, ShellSystem};

pub(super) fn run<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    if args.is_empty() {
        shell.println("usage: rm FILE...");
        return;
    }

    for &arg in args {
        let path = shell.make_absolute_path(Some(arg));
        match shell.fs.remove_file(&path) {
            Ok(()) => {}
            Err(FsError::Unavailable) => {
                shell.println("rm: filesystem unavailable");
                return;
            }
            Err(FsError::NotFound) => {
                shell.print("rm: cannot remove '");
                shell.print(&path);
                shell.println("': No such file");
            }
            Err(FsError::NotFile) => {
                shell.print("rm: cannot remove '");
                shell.print(&path);
                shell.println("': Is a directory");
            }
            Err(FsError::NotDirectory) => {
                shell.print("rm: parent is not a directory: ");
                shell.println(&path);
            }
            Err(FsError::PermissionDenied) => shell.println("rm: permission denied"),
            Err(FsError::Corrupt) => shell.println("rm: filesystem corrupt"),
            Err(FsError::AlreadyExists) | Err(FsError::DirectoryNotEmpty) => {
                shell.println("rm: filesystem error");
            }
        }
    }
}
