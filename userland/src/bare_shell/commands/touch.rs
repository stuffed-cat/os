use super::super::{BareShell, FsError, ShellFs, ShellIo, ShellSystem};

pub(super) fn run<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    if args.is_empty() {
        shell.println("usage: touch PATH...");
        return;
    }

    for &arg in args {
        let path = shell.make_absolute_path(Some(arg));
        match shell.fs.create_file(&path, 0o666) {
            Ok(()) | Err(FsError::AlreadyExists) => {}
            Err(FsError::Unavailable) => {
                shell.println("touch: filesystem unavailable");
                return;
            }
            Err(FsError::NotFound) => {
                shell.print("touch: parent directory not found: ");
                shell.println(&path);
            }
            Err(FsError::NotDirectory) => {
                shell.print("touch: parent is not a directory: ");
                shell.println(&path);
            }
            Err(FsError::PermissionDenied) => shell.println("touch: permission denied"),
            Err(FsError::NotFile) => {
                shell.print("touch: cannot operate on non-regular file: ");
                shell.println(&path);
            }
            Err(FsError::Corrupt) => shell.println("touch: filesystem corrupt"),
            Err(FsError::DirectoryNotEmpty) => {
                shell.println("touch: filesystem error");
            }
        }
    }
}
