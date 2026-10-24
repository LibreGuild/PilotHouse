//! Git on a mapped network drive, against a real SSH server. Skipped unless
//! `PH_SSH_TEST` is set; run it with a server that shares a folder with this
//! computer (docs/testing.md has a Docker recipe):
//!
//! ```sh
//! PH_SSH_TEST=1 PH_SSH_HOST=dev@127.0.0.1 PH_SSH_PORT=2222 \
//! PH_SSH_LOCAL=/path/to/share PH_SSH_REMOTE=/srv/share \
//! PH_SSH_STATE=/path/to/pilothouse/ssh cargo test -p ph-git --test network_drive
//! ```

use std::path::PathBuf;

fn setup() -> Option<(ph_remote::Mapping, PathBuf)> {
    std::env::var_os("PH_SSH_TEST")?;
    let var = |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"));
    let state = PathBuf::from(var("PH_SSH_STATE"));
    ph_remote::configure(state.clone(), state.join("cm"));
    let mapping = ph_remote::Mapping {
        local: var("PH_SSH_LOCAL"),
        resolved: String::new(),
        host: var("PH_SSH_HOST"),
        port: std::env::var("PH_SSH_PORT")
            .ok()
            .map(|port| port.parse().unwrap()),
        remote: var("PH_SSH_REMOTE"),
    };
    ph_remote::set_mappings(vec![mapping.clone()]);
    let root = PathBuf::from(&mapping.local).join(format!("ph-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    Some((mapping, root))
}

#[test]
fn git_runs_on_the_server_and_its_paths_come_back() {
    let Some((mapping, root)) = setup() else {
        return;
    };
    let git = |args: &[&str]| ph_git::git(&root, args).unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.email", "test@example.test"]);
    git(&["config", "user.name", "Test"]);
    // The server says where it ran: its own folder, not this computer's.
    let remote = git(&["rev-parse", "--show-toplevel"]);
    assert!(remote.trim().starts_with(&mapping.remote), "{remote}");

    std::fs::write(root.join("README.md"), "# Test\n").unwrap();
    git(&["add", "README.md"]);
    git(&["commit", "-q", "-m", "init"]);
    std::fs::write(root.join("README.md"), "# Test\nchanged\n").unwrap();
    std::fs::write(root.join("new.txt"), "new\n").unwrap();
    let status = ph_git::status(&root).unwrap();
    assert_eq!(status.branch.as_deref(), Some("main"));
    assert_eq!(status.changes.len(), 2, "{status:?}");

    // Checkpoints: a temporary index on the drive, Git on the server.
    let checkpoint = ph_git::checkpoint::create(&root, "test").unwrap().unwrap();
    std::fs::write(root.join("README.md"), "gone\n").unwrap();
    std::fs::remove_file(root.join("new.txt")).unwrap();
    std::fs::write(root.join("later.txt"), "later\n").unwrap();
    ph_git::checkpoint::restore(&root, &checkpoint).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        "# Test\nchanged\n"
    );
    assert!(root.join("new.txt").exists() && !root.join("later.txt").exists());

    // Extra working trees go in the share, and list as this computer's paths.
    let home = ph_git::worktree::home(&root, "test-install").unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let lane = home.join(format!("lane-{}", std::process::id()));
    let head = git(&["rev-parse", "HEAD"]);
    ph_git::worktree::add_detached(&root, &lane, head.trim()).unwrap();
    assert!(
        lane.join("README.md").exists(),
        "made on the server, seen here"
    );
    let listed = ph_git::worktree::list(&root).unwrap();
    assert!(listed.iter().any(|tree| tree.path == lane), "{listed:?}");
    ph_git::worktree::remove(&root, &lane, true).unwrap();
    assert!(!lane.exists());

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(PathBuf::from(&mapping.local).join(".pilothouse"));
}
