use alloc::format;

use super::super::{BareShell, FsError, ShellFs, ShellIo, ShellSystem};

pub(super) fn run<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    if args.len() != 2 {
        shell.println("usage: mv SOURCE DEST");
        return;
    }

    let src_path = shell.make_absolute_path(Some(args[0]));
    let mut dest_path = shell.make_absolute_path(Some(args[1]));

    match shell.fs.read_file(&src_path) {
        Ok(data) => {
            let dest_is_directory = match shell.fs.list_dir(&dest_path) {
                Ok(_) => Some(true),
                Err(FsError::NotFound) | Err(FsError::NotDirectory) | Err(FsError::NotFile) => {
                    Some(false)
                }
                Err(FsError::Unavailable) => {
                    shell.println("mv: filesystem unavailable");
                    return;
                }
                Err(FsError::PermissionDenied) => {
                    shell.println("mv: permission denied");
                    return;
                }
                Err(FsError::Corrupt) => {
                    shell.println("mv: filesystem corrupt");
                    return;
                }
                Err(FsError::DirectoryNotEmpty) => Some(false),
                Err(_) => {
                    shell.println("mv: filesystem error");
                    return;
                }
            };

            let dest_is_directory = dest_is_directory.unwrap_or(false);
            if dest_is_directory {
                let Some(name) = src_path.rsplit('/').find(|component| !component.is_empty())
                else {
                    shell.println("mv: invalid source path");
                    return;
                };
                dest_path = if dest_path == "/" {
                    format!("/{name}")
                } else {
                    format!("{dest_path}/{name}")
                };
                dest_path = shell.normalize_path(&dest_path);
            }

            if src_path == dest_path {
                shell.println("mv: source and destination are the same");
                return;
            }

            match shell.fs.create_file(&dest_path, 0o644) {
                Ok(()) | Err(FsError::AlreadyExists) => {}
                Err(FsError::Unavailable) => {
                    shell.println("mv: filesystem unavailable");
                    return;
                }
                Err(FsError::NotFound) => {
                    shell.print("mv: destination directory not found: ");
                    shell.println(&dest_path);
                    return;
                }
                Err(FsError::NotDirectory) => {
                    shell.print("mv: destination parent is not a directory: ");
                    shell.println(&dest_path);
                    return;
                }
                Err(FsError::NotFile) => {
                    shell.print("mv: destination is not a regular file: ");
                    shell.println(&dest_path);
                    return;
                }
                Err(FsError::PermissionDenied) => {
                    shell.println("mv: permission denied");
                    return;
                }
                Err(FsError::Corrupt) => {
                    shell.println("mv: filesystem corrupt");
                    return;
                }
                Err(FsError::DirectoryNotEmpty) => {
                    shell.println("mv: filesystem error");
                    return;
                }
            }

            match shell.fs.write_file(&dest_path, 0, &data, true) {
                Ok(bytes) => {
                    if bytes != data.len() {
                        shell.println("mv: incomplete write");
                    }
                }
                Err(FsError::Unavailable) => {
                    shell.println("mv: filesystem unavailable");
                    return;
                }
                Err(FsError::NotFound) => {
                    shell.print("mv: destination not found: ");
                    shell.println(&dest_path);
                    return;
                }
                Err(FsError::NotDirectory) => {
                    shell.print("mv: destination parent is not a directory: ");
                    shell.println(&dest_path);
                    return;
                }
                Err(FsError::NotFile) => {
                    shell.print("mv: destination is not a regular file: ");
                    shell.println(&dest_path);
                    return;
                }
                Err(FsError::PermissionDenied) => {
                    shell.println("mv: permission denied");
                    return;
                }
                Err(FsError::Corrupt) => {
                    shell.println("mv: filesystem corrupt");
                    return;
                }
                Err(FsError::AlreadyExists) | Err(FsError::DirectoryNotEmpty) => {
                    shell.println("mv: filesystem error");
                    return;
                }
            }

            match shell.fs.remove_file(&src_path) {
                Ok(()) => {}
                Err(FsError::NotFound) => {}
                Err(FsError::Unavailable) => shell.println("mv: filesystem unavailable"),
                Err(FsError::PermissionDenied) => shell.println("mv: permission denied"),
                Err(FsError::NotFile) => {
                    shell.println("mv: source is not a regular file");
                }
                Err(FsError::NotDirectory) => {
                    shell.println("mv: source parent is not a directory");
                }
                Err(FsError::Corrupt) => shell.println("mv: filesystem corrupt"),
                Err(FsError::AlreadyExists) | Err(FsError::DirectoryNotEmpty) => {
                    shell.println("mv: filesystem error");
                }
            }
        }
        Err(FsError::Unavailable) => shell.println("mv: filesystem unavailable"),
        Err(FsError::NotFound) => {
            shell.print("mv: no such file: ");
            shell.println(&src_path);
        }
        Err(FsError::NotFile) | Err(FsError::NotDirectory) => {
            shell.println("mv: source is not a regular file");
        }
        Err(FsError::PermissionDenied) => {
            shell.println("mv: permission denied");
        }
        Err(FsError::Corrupt) => {
            shell.println("mv: filesystem corrupt");
        }
        Err(_) => {
            shell.println("mv: filesystem error");
        }
    }
}
