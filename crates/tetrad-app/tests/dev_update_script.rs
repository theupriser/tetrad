//! `scripts/dev-update.sh` (docs/development.md, "Your own updates"): the manifest and signature
//! it writes are the ones the unit checks with `Manifest::verify`, with the key it prints.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::process::Command;

use tetrad_services::updater::tetrad::Manifest;

const SCRIPT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/dev-update.sh");

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
