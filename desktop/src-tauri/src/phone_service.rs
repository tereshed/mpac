//! A per-user launchd job owns remote access, independently of the GUI.
//! No PID read from disk is ever signalled. Stop addresses our launchd label.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone)]
pub struct Service {
    pub label: String,
    plist: PathBuf,
}
impl Service {
    pub fn new(profile: &Path, data: &Path, ip: &str) -> Result<Self> {
        let home = std::env::var_os("HOME").context("Cannot locate LaunchAgents")?;
        let identity = format!("{}\n{}\n{ip}", profile.display(), data.display());
        let label = format!(
            "com.mpac.phone.{}",
            mpac_mobile_link::secret_hash(&identity)
        );
        Ok(Self {
            plist: PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
            label,
        })
    }
    fn target(&self) -> String {
        format!("gui/{}/{}", unsafe { libc::geteuid() }, self.label)
    }
    pub fn pid(&self) -> Option<u32> {
        let output = Command::new("/bin/launchctl")
            .args(["print", &self.target()])
            .output()
            .ok()?;
        output.status.success().then_some(())?;
        job_pid(&String::from_utf8_lossy(&output.stdout))
    }
    pub fn start(&self, executable: &Path, args: &[String]) -> Result<()> {
        let folder = self.plist.parent().unwrap();
        if !folder.exists() {
            crate::settings::private_directory(folder)?;
        }
        use std::os::unix::fs::MetadataExt;
        let meta = fs::symlink_metadata(folder)?;
        ensure!(
            meta.is_dir()
                && !meta.file_type().is_symlink()
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.mode() & 0o022 == 0,
            "Unsafe LaunchAgents directory"
        );
        let contents = plist(&self.label, executable, args)?;
        if self.plist.exists() {
            let previous = super::phone::read_launch_bytes(&self.plist, 32768)?;
            ensure!(
                previous == contents.as_bytes(),
                "Remote access service configuration changed; stop mobile access before updating it"
            );
        } else {
            super::phone::private_save(&self.plist, contents.as_bytes())?;
        }
        if self.pid().is_none() {
            // A loaded job may be waiting for its interface or restart throttle.
            let loaded = Command::new("/bin/launchctl")
                .args(["print", &self.target()])
                .output()?;
            if !loaded.status.success() {
                let output = Command::new("/bin/launchctl")
                    .arg("bootstrap")
                    .arg(format!("gui/{}", unsafe { libc::geteuid() }))
                    .arg(&self.plist)
                    .output()?;
                ensure!(
                    output.status.success(),
                    "Could not enable background mobile access"
                );
            }
        }
        Ok(())
    }
    pub fn stop(&self) -> Result<()> {
        let loaded = Command::new("/bin/launchctl")
            .args(["print", &self.target()])
            .output()?;
        if loaded.status.success() {
            ensure!(
                Command::new("/bin/launchctl")
                    .args(["bootout", &self.target()])
                    .output()?
                    .status
                    .success(),
                "Could not stop background mobile access"
            );
        }
        if self.plist.exists() {
            // Check ownership and refuse symlinks before deleting our registration.
            super::phone::read_launch_bytes(&self.plist, 32768)?;
            fs::remove_file(&self.plist)?;
        }
        Ok(())
    }
}
fn xml(value: &str) -> Result<String> {
    ensure!(
        !value.chars().any(|c| c.is_control()),
        "Invalid service argument"
    );
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}
fn plist(label: &str, executable: &Path, args: &[String]) -> Result<String> {
    ensure!(
        executable.is_absolute(),
        "Service executable must be absolute"
    );
    let mut arguments = format!(
        "<string>{}</string>",
        xml(executable.to_str().context("Invalid executable path")?)?
    );
    for arg in args {
        arguments.push_str(&format!("<string>{}</string>", xml(arg)?));
    }
    Ok(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array>{arguments}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>ProcessType</key><string>Background</string></dict></plist>",
        xml(label)?
    ))
}
fn job_pid(output: &str) -> Option<u32> {
    output.lines().find_map(|line| {
        line.trim()
            .strip_prefix("pid = ")?
            .parse::<u32>()
            .ok()
            .filter(|p| *p > 1)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_a_running_job_pid() {
        assert_eq!(
            job_pid("service = {\n state = running\n pid = 12345\n}"),
            Some(12345)
        );
        for value in [
            "last exit code = 123",
            "pid = 0",
            "pid = -1",
            "environment = { pid = 123 }",
        ] {
            assert_eq!(job_pid(value), None);
        }
    }
    #[test]
    fn launch_arguments_are_literal_xml_and_never_shell_commands() {
        let value = plist(
            "test",
            Path::new("/Applications/My App.app/helper"),
            &["/tmp/a & <b> \"x\" $(touch nope)".into()],
        )
        .unwrap();
        assert!(value.contains("/tmp/a &amp; &lt;b&gt; &quot;x&quot; $(touch nope)"));
        assert!(!value.contains("/bin/sh"));
        assert!(plist("test", Path::new("relative"), &[]).is_err());
        assert!(xml("bad\nargument").is_err());
    }
    /// Run only against a disposable daemon via MPAC_TEST_HOST / MPAC_TEST_DATA.
    #[test]
    #[ignore = "explicit isolated launchd/TLS integration test"]
    fn background_gateway_recovers_and_stop_disables_autostart() {
        use std::{
            thread,
            time::{Duration, Instant},
        };
        let executable =
            PathBuf::from(std::env::var_os("MPAC_TEST_HOST").expect("test host executable"));
        let data =
            PathBuf::from(std::env::var_os("MPAC_TEST_DATA").expect("disposable daemon data"));
        assert!(data.to_string_lossy().contains("tailscale-mac-qa"));
        let root = data
            .parent()
            .unwrap()
            .join(format!("gateway-{}", uuid::Uuid::new_v4()));
        crate::settings::private_directory(&root).unwrap();
        let job = Service::new(&root, &data, "127.0.0.1").unwrap();
        struct Cleanup(Service);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = self.0.stop();
            }
        }
        let _cleanup = Cleanup(job.clone());
        let args = vec![
            "--data-dir".into(),
            data.to_string_lossy().into_owned(),
            "--state-dir".into(),
            root.to_string_lossy().into_owned(),
            "--bind".into(),
            "127.0.0.1:0".into(),
        ];
        let ready = |job: &Service, previous: Option<u32>| {
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(pid) = job.pid()
                    && Some(pid) != previous
                {
                    if let Ok(value) =
                        crate::phone::read_launch_bytes(&root.join("launch.json"), 8192).and_then(
                            |bytes| Ok(serde_json::from_slice::<serde_json::Value>(&bytes)?),
                        )
                    {
                        if value["pid"].as_u64() == Some(pid as u64) {
                            let admin =
                                url::Url::parse(value["admin_url"].as_str().unwrap()).unwrap();
                            if let Ok(reply) = crate::connection::companion_request(
                                pid,
                                &admin,
                                "/admin/state",
                                b"{}",
                            ) && reply.status == 200
                            {
                                break (pid, admin);
                            }
                        }
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "background gateway did not become ready"
                );
                thread::sleep(Duration::from_millis(200));
            }
        };
        job.start(&executable, &args).unwrap();
        let (first, old_admin) = ready(&job, None);
        let certificate = fs::read(root.join("tls.json")).unwrap();
        // Dropping a GUI-side handle must not stop the job.
        drop(job.clone());
        assert_eq!(job.pid(), Some(first));
        // Recreating the manager recovers the same job, rather than spawning two.
        let recreated = Service::new(&root, &data, "127.0.0.1").unwrap();
        recreated.start(&executable, &args).unwrap();
        assert_eq!(recreated.pid(), Some(first));
        assert!(
            Command::new("/bin/launchctl")
                .args(["kickstart", "-k", &job.target()])
                .status()
                .unwrap()
                .success()
        );
        let (second, _) = ready(&recreated, Some(first));
        assert_ne!(first, second);
        assert_eq!(certificate, fs::read(root.join("tls.json")).unwrap());
        assert!(
            crate::connection::companion_request(first, &old_admin, "/admin/state", b"{}").is_err()
        );
        recreated.stop().unwrap();
        assert_eq!(recreated.pid(), None);
        assert!(!recreated.plist.exists());
        thread::sleep(Duration::from_secs(1));
        assert_eq!(recreated.pid(), None);
    }
}
