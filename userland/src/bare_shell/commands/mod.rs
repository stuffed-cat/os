pub(super) mod cp;
pub(super) mod mkdir;
pub(super) mod mv;
pub(super) mod rm;
pub(super) mod rmdir;
pub(super) mod touch;

use super::{BareShell, ShellFs, ShellIo, ShellSystem};

pub(super) fn cp<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    cp::run(shell, args);
}

pub(super) fn touch<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    touch::run(shell, args);
}

pub(super) fn mkdir<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    mkdir::run(shell, args);
}

pub(super) fn rmdir<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    rmdir::run(shell, args);
}

pub(super) fn rm<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    rm::run(shell, args);
}

pub(super) fn mv<Io, Fs, Sys>(shell: &mut BareShell<Io, Fs, Sys>, args: &[&str])
where
    Io: ShellIo,
    Fs: ShellFs,
    Sys: ShellSystem,
{
    mv::run(shell, args);
}
