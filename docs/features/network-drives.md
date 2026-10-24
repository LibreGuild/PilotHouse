# Network drives (Git, terminals and runs over SSH)

Crate: `ph-remote`, with the settings in `workspace/src/network_drives.rs`.

Many teams keep code on a Linux server and reach it as a network drive: a Samba share mapped to `Z:` on Windows, or a share mounted on macOS or Linux. Opening such a folder works like any other, but Git, shells and builds would then run on this computer, over the network, on another system's files. A **network drive** tells PilotHouse that a folder here is a server's folder, so that work runs **on the server**, over SSH, next to the files.

| Runs on the server | Runs on this computer |
|---|---|
| Git (status, diffs, commits, branches, history, blame, stashes, conflicts, checkpoints) | Reading and saving files (through the drive) |
| Terminals (a shell on the server, in the same folder) | The file tree, editor, ⌘P and search |
| Run configurations (they run in a terminal) | Agents (Claude Code, Codex, Gemini CLI, Antigravity) |
| The ports panel (the server's ports) | Language servers |

## Setting one up

**Settings → Network drives**, the palette (**Network drives…**), or the **Network drive** chip in the status bar of a workspace on one (it fills in the server and share):

1. **The folder here**: the drive or folder (`Z:\`, `\\server\share`, `/Volumes/code`). **Choose…** picks it and reads the server from the system.
2. **The server**: `user@server`, or a host from `~/.ssh/config` (aliases, jump hosts and keys there apply), and the port if it is not 22.
3. **The same folder on the server** (`/srv/code`). Left empty, PilotHouse asks Samba on the server (`testparm`) where the share is, by the share's name (from the drive, else the folder's own name).
4. **Check and save** goes through:
   - **The server's key.** A server new to this computer shows its key fingerprint; trust it if it is the one its admin gives. `ssh` itself fetches the key, so jump hosts and host-key aliases in `~/.ssh/config` apply. Trusted servers go in PilotHouse's own `known_hosts` (`<data dir>/ssh/known_hosts`); the `known_hosts` files your SSH config names count too. A server whose key changed is not trusted again silently: after asking its admin, **Forget the old key** removes PilotHouse's copy (and gives the `ssh-keygen -R` command when the old key is in your own files).
   - **Signing in.** PilotHouse uses your SSH keys and agent (the one your login shell uses) and never stores a password. The check and the key setup use a connection of their own, so only a working key passes. When the server refuses your key, **Install my key on the server** opens a terminal in the dialog where you sign in with your password once; with no key yet, **Make a key** runs `ssh-keygen` there first. A key with a passphrase must be in your SSH agent: **Add my key to the SSH agent** runs `ssh-add` (on macOS, into the keychain). On Windows that needs the *OpenSSH Authentication Agent* service running.
   - **The same folder.** A small file written here must appear on the server; it is removed right after. This catches a wrong path before anything runs in it.

Every workspace in the folder, and any opened later, then shows **SSH · server** in the status bar.

## How it works

- **Commands.** A command that would run in a mapped folder (`git …`) becomes `ssh -T -o BatchMode=yes … server 'cd /srv/code/app && exec git …'`. Arguments and variables that are paths under the drive (Git's temporary index for checkpoints, a new worktree's folder) become the server's paths, and paths Git prints become this computer's again. Commands never prompt: one that cannot sign in fails, and the Git tab says why ("Cannot reach…", "did not accept your SSH key", "not trusted yet").
- **One connection per server** on macOS and Linux (`ControlMaster`), so a Git refresh does not sign in again; it closes when PilotHouse quits, or after ten idle minutes. OpenSSH for Windows cannot share connections, so there each Git command signs in (a refresh runs about five).
- **Terminals** start `ssh -t server`, which may ask for a password or to accept a new server like any terminal, and land in the same folder. One that cannot connect keeps its tab, showing why.
- **Ports** come from `ss` on the server (Linux); agents asking for ports get the same. **Open** or **Copy** makes a tunnel (`ssh -L`, on a connection of its own) from a port here, the same number when it is free, to the address the server listens on (`::1` too), and opens `http://127.0.0.1:<port>`. A tunnel's `ssh` reads its input from PilotHouse, so it ends when PilotHouse does, however it ends.
- **Copies for agents** (a pull request checked out for review, parallel-run lanes) go on the share, in `.pilothouse/<install>/worktrees`, where Git on the server can make them and agents here can reach them; their link to the repository is relative, so Git works in them from both sides. Each install has its own folder, and cleaning up only touches it, so people sharing a drive never remove each other's. When the share is the repository itself, the folder is kept out of Git (`info/exclude`).
- **Where repositories are** is read from the drive (a `.git` up the tree), so a server that is down reads as down, not as "not a repository".
- On Windows, a folder opened from a mapped drive keeps its letter (`Z:\app`) rather than becoming `\\server\share\app`: `cmd.exe` cannot start in a share.

## Limits

- The server needs `git` (the check says if it has none) and, for the ports panel, `ss`.
- Your login shell on the server should print nothing for non-interactive sessions (a `.bashrc` that echoes would end up in Git's output).
- Agents run here, against the drive: their own shell commands (builds, tests) run on this computer.
- Search and the file index read through the drive, so a very large share is slower to search than a local disk.
