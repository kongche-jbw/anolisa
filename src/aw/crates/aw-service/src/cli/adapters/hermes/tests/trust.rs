use super::*;

#[test]
fn profile_checks_optional_dotenv_without_following_links() {
    let profile = Profile::new("unknown: keep\n");
    let path = profile.0.to_str().unwrap();
    let dotenv = profile.0.join(".env");
    assert!(install::profile(path).is_ok());
    fs::write(&dotenv, "SENTINEL=keep\n").unwrap();
    for mode in [0o600, 0o644, 0o664, 0o666] {
        fs::set_permissions(&dotenv, fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(install::profile(path).is_ok(), mode & 0o022 == 0);
        assert_eq!(fs::read(&dotenv).unwrap(), b"SENTINEL=keep\n");
    }
    fs::remove_file(&dotenv).unwrap();
    for target in [profile.0.join("config.yaml"), profile.0.join("missing")] {
        symlink(target, &dotenv).unwrap();
        assert!(install::profile(path)
            .unwrap_err()
            .to_string()
            .contains(".env"));
        fs::remove_file(&dotenv).unwrap();
    }
    fs::create_dir(&dotenv).unwrap();
    assert!(install::profile(path).is_err());
    fs::remove_dir(&dotenv).unwrap();
    let name = std::ffi::CString::new(dotenv.to_str().unwrap()).unwrap();
    // SAFETY: name is a valid C string inside this test's owned directory.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(install::profile(path).is_err());
}
