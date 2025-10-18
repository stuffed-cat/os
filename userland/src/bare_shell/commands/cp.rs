use alloc::format;

use super::super::{BareShell, FsError, ShellFs, ShellIo, ShellSystem};

pub(super) fn run<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    if args.is_empty() {
        shell.println("cp: missing file operand");
        return;
    }
    if args.len() == 1 {
        shell.print("cp: missing destination file operand after '");
        shell.print(args[0]);
        shell.println("'");
        return;
    }
    if args.len() > 2 {
        shell.println("cp: multiple sources are not supported in the bare shell yet");
        return;
    }

    let src_arg = args[0];
    let dest_arg = args[1];
    let src_path = shell.make_absolute_path(Some(src_arg));
    let mut dest_path = shell.make_absolute_path(Some(dest_arg));

    let data = match shell.fs.read_file(&src_path) {
        Ok(bytes) => bytes,
        Err(FsError::Unavailable) => {
            shell.println("cp: filesystem unavailable");
            return;
        }
        Err(FsError::NotFound) => {
            shell.print("cp: no such file: ");
            shell.println(src_arg);
            return;
        }
        Err(FsError::NotFile) | Err(FsError::NotDirectory) => {
            shell.print("cp: not a regular file: ");
            shell.println(src_arg);
            return;
        }
        Err(FsError::PermissionDenied) => {
            shell.println("cp: permission denied");
            return;
        }
        Err(FsError::Corrupt) => {
            shell.println("cp: filesystem corrupt");
            return;
        }
        Err(_) => {
            shell.println("cp: filesystem error");
            return;
        }
    };

    let dest_is_directory = match shell.fs.list_dir(&dest_path) {
        Ok(_) => Some(true),
        Err(FsError::NotFound) | Err(FsError::NotDirectory) | Err(FsError::NotFile) => Some(false),
        Err(FsError::Unavailable) => {
            shell.println("cp: filesystem unavailable");
            None
        }
        Err(FsError::PermissionDenied) => {
            shell.println("cp: permission denied");
            None
        }
        Err(FsError::Corrupt) => {
            shell.println("cp: filesystem corrupt");
            None
        }
        Err(_) => {
            shell.println("cp: filesystem error");
            None
        }
    };

    let Some(dest_is_directory) = dest_is_directory else {
        return;
    };

    if dest_is_directory {
        let Some(name) = src_path.rsplit('/').find(|component| !component.is_empty()) else {
            shell.println("cp: invalid source path");
            return;
        };
        let combined = if dest_path == "/" {
            format!("/{name}")
        } else {
            format!("{dest_path}/{name}")
        };
        dest_path = shell.normalize_path(&combined);
    }

    if src_path == dest_path {
        shell.println("cp: source and destination are the same file");
        return;
    }

    match shell.fs.create_file(&dest_path, 0o644) {
        Ok(()) | Err(FsError::AlreadyExists) => {}
        Err(FsError::Unavailable) => {
            shell.println("cp: filesystem unavailable");
            return;
        }
        Err(FsError::NotFound) => {
            shell.print("cp: destination directory not found: ");
            shell.println(&dest_path);
            return;
        }
        Err(FsError::NotDirectory) => {
            shell.print("cp: destination parent is not a directory: ");
            shell.println(&dest_path);
            return;
        }
        Err(FsError::NotFile) => {
            shell.print("cp: destination is not a regular file: ");
            shell.println(&dest_path);
            return;
        }
        Err(FsError::PermissionDenied) => {
            shell.println("cp: permission denied");
            return;
        }
        Err(FsError::Corrupt) => {
            shell.println("cp: filesystem corrupt");
            return;
        }
        Err(FsError::DirectoryNotEmpty) => {
            shell.println("cp: filesystem error");
            return;
        }
    }

    match shell.fs.write_file(&dest_path, 0, &data, true) {
        Ok(written) => {
            if written != data.len() {
                shell.println("cp: incomplete write");
            }
        }
        Err(FsError::Unavailable) => shell.println("cp: filesystem unavailable"),
        Err(FsError::NotFound) => {
            shell.print("cp: destination not found: ");
            shell.println(&dest_path);
        }
        Err(FsError::NotDirectory) => {
            shell.print("cp: destination parent is not a directory: ");
            shell.println(&dest_path);
        }
        Err(FsError::NotFile) => {
            shell.print("cp: destination is not a regular file: ");
            shell.println(&dest_path);
        }
        Err(FsError::PermissionDenied) => shell.println("cp: permission denied"),
        Err(FsError::Corrupt) => shell.println("cp: filesystem corrupt"),
        Err(FsError::AlreadyExists) => shell.println("cp: filesystem error"),
        Err(FsError::DirectoryNotEmpty) => shell.println("cp: filesystem error"),
    }
}
