//! `scripts/dev-update.sh` (docs/development.md, "Your own updates"): the manifest and signature
//! it writes are the ones the unit checks with `Manifest::verify`, with the key it prints.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;

use tetrad_services::updater::tetrad::Manifest;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/dev-update.sh");
const BUILD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/build-dev.sh");

fn run(args: &[&Path]) -> Option<String> {
    let output = Command::new("sh").arg(SCRIPT).args(args).output().ok()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).unwrap())
}

#[test]
fn a_packed_update_verifies_with_the_printed_key_only() {
    if Command::new("openssl").arg("version").output().is_err() {
        eprintln!("skipped: no openssl");
        return;
    }
    let dir = std::env::temp_dir().join(format!("tetrad-dev-update-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let (key, other) = (dir.join("key.pem"), dir.join("other.pem"));
    let public = |out: &str| {
        let line = out.lines().find(|l| l.starts_with("public key: "));
        line.unwrap().trim_start_matches("public key: ").to_owned()
    };
    let public_key = public(&run(&[Path::new("key"), &key]).unwrap());
    let other_key = public(&run(&[Path::new("key"), &other]).unwrap());
    assert_ne!(public_key, other_key);
    // The same key again: nothing is made, the same key is printed.
    assert_eq!(public(&run(&[Path::new("key"), &key]).unwrap()), public_key);

    let build = dir.join("build");
    fs::create_dir_all(&build).unwrap();
    fs::write(build.join("tetrad"), vec![7_u8; 1234]).unwrap();
    run(&[Path::new("pack"), &build, &key]).unwrap();

    let manifest = fs::read(build.join("tetrad-update.json")).unwrap();
    let signature = fs::read(build.join("tetrad-update.json.sig")).unwrap();
    let verified = Manifest::verify(&manifest, &signature, &public_key, None).unwrap();
    assert_eq!(verified.version.to_string(), env!("CARGO_PKG_VERSION"));
    assert_eq!(verified.files["bin/tetrad"].size, 1234);
    for (path, entry) in &verified.files {
        let name = path.rsplit('/').next().unwrap();
        let bytes = fs::read(build.join(name)).unwrap();
        assert_eq!(entry.size, bytes.len() as u64, "{path}");
    }
    // Another key, or a changed manifest, does not verify.
    assert!(Manifest::verify(&manifest, &signature, &other_key, None).is_err());
    let changed = String::from_utf8(manifest).unwrap().replace("1234", "1235");
    assert!(Manifest::verify(changed.as_bytes(), &signature, &public_key, None).is_err());
    fs::remove_dir_all(&dir).unwrap();
}

/// The names in a tar.gz, in order.
fn listing(archive: &Path) -> Vec<String> {
    let output = Command::new("tar")
        .arg("-tzf")
        .arg(archive)
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `archive` makes one file: the binary alone, or with the three launcher scripts. The files
/// are flat (the Remote page takes them by name) and the contents are the build's.
#[test]
fn an_archive_has_the_binary_and_with_scripts_the_launchers() {
    let dir = std::env::temp_dir().join(format!("tetrad-dev-archive-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let binary = dir.join("tetrad");
    fs::write(&binary, vec![5_u8; 777]).unwrap();

    let default = dir.join("tetrad-update.tar.gz");
    run(&[Path::new("archive"), &binary]).unwrap();
    assert_eq!(listing(&default), ["tetrad"]);

    let all = dir.join("all.tar.gz");
    run(&[Path::new("archive"), &binary, Path::new("--scripts"), &all]).unwrap();
    let names = [
        "tetrad",
        "tetrad-launch.sh",
        "tetrad-launcher.sh",
        "tetrad-update.sh",
    ];
    assert_eq!(listing(&all), names);
    let out = dir.join("out");
    fs::create_dir_all(&out).unwrap();
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(&all)
        .arg("-C")
        .arg(&out)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read(out.join("tetrad")).unwrap(), vec![5_u8; 777]);
    let launcher = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/launcher/tetrad-launcher.sh"
    );
    assert_eq!(
        fs::read(out.join("tetrad-launcher.sh")).unwrap(),
        fs::read(launcher).unwrap()
    );
    fs::remove_dir_all(&dir).unwrap();
}

/// `build-dev.sh` runs `build-arm.sh` with `--features dev` on its `zigbuild` call only, and
/// leaves `tetrad-update.tar.gz` beside the binary.
#[test]
fn a_dev_build_ends_with_the_update_archive() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("tetrad-dev-build-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let (bin, out) = (dir.join("bin"), dir.join("out"));
    fs::create_dir_all(&bin).unwrap();
    let calls = dir.join("calls");
    let write = |path: &Path, text: &str| {
        fs::write(path, text).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    // The cargo-zigbuild of the environment records its arguments; build-arm.sh calls it as the
    // real one does, once for the version and once to build, and writes the binary.
    write(
        &bin.join("cargo-zigbuild"),
        &format!("#!/bin/sh\necho \"$*\" >>{}\n", calls.display()),
    );
    let build_arm = dir.join("build-arm.sh");
    write(
        &build_arm,
        "#!/bin/sh\ncargo-zigbuild --version\n\
         cargo-zigbuild zigbuild --locked --release -p tetrad-app --bin tetrad\n\
         mkdir -p \"$2\" && echo elf >\"$2/tetrad\"\n",
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = Command::new("sh")
        .arg(BUILD)
        .arg("--scripts")
        .arg(&out)
        .env("PATH", path)
        .env("TD_BUILD_ARM", &build_arm)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(&calls).unwrap();
    let lines: Vec<_> = calls.lines().collect();
    assert_eq!(
        lines,
        [
            "--version",
            "zigbuild --locked --release -p tetrad-app --bin tetrad --features dev"
        ]
    );
    assert_eq!(
        listing(&out.join("tetrad-update.tar.gz")),
        [
            "tetrad",
            "tetrad-launch.sh",
            "tetrad-launcher.sh",
            "tetrad-update.sh"
        ]
    );
    fs::remove_dir_all(&dir).unwrap();
}
