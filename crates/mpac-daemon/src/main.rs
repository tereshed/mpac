use std::{
    fs,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use mpac_daemon::{
    api::{self, App},
    model::{self, Project},
    store::Store,
};
use serde_json::json;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|v| v == "--agent-event") {
        mpac_daemon::activity_hooks::run_helper();
        return Ok(());
    }
    if args.first().is_some_and(|v| v == "--demo-agent") {
        return demo_agent();
    }
    if args.first().is_some_and(|v| v == "--demo-leaf") {
        std::thread::sleep(Duration::from_secs(30));
        return Ok(());
    }
    if args.first().is_some_and(|v| v == "--demo-branch") {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--demo-leaf")
            .spawn()?;
        println!("[demo-grandchild {}]\r", child.id());
        io::stdout().flush()?;
        child.wait()?;
        return Ok(());
    }
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(run(args))
}

#[derive(Debug, PartialEq, Eq)]
struct DaemonOptions {
    data: PathBuf,
    ui: PathBuf,
    port: u16,
}

fn daemon_options(
    args: Vec<String>,
    checkout_root: impl FnOnce() -> Result<PathBuf>,
) -> Result<Option<DaemonOptions>> {
    let mut data = None;
    let mut ui = None;
    let mut port = 4317;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                data = Some(PathBuf::from(args.next().context("Нужен путь --data-dir")?))
            }
            "--ui-dir" => ui = Some(PathBuf::from(args.next().context("Нужен путь --ui-dir")?)),
            "--port" => port = args.next().context("Нужен --port")?.parse::<u16>()?,
            "--help" => return Ok(None),
            _ => bail!("Неизвестный аргумент"),
        }
    }
    // Packaged applications supply both paths. Resolve development defaults
    // only when needed, so a shipped binary never requires its build checkout.
    if data.is_none() || ui.is_none() {
        let root = checkout_root()?;
        data.get_or_insert_with(|| root.join(".mpac"));
        ui.get_or_insert_with(|| root.join("ui/dist"));
    }
    Ok(Some(DaemonOptions {
        data: data.unwrap(),
        ui: ui.unwrap(),
        port,
    }))
}

async fn run(args: Vec<String>) -> Result<()> {
    let Some(DaemonOptions { data, ui, port }) = daemon_options(args, || {
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()?)
    })?
    else {
        println!("mpac-daemon [--data-dir PATH] [--ui-dir PATH] [--port 4317]");
        return Ok(());
    };
    fs::create_dir_all(&data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&data, fs::Permissions::from_mode(0o700))?;
    }
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(data.join("daemon.lock"))?;
    lock.try_lock_exclusive()
        .context("Этот data-dir уже используется другим daemon")?;
    // Bind only loopback, before mutating any persisted session state.
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .context("Локальный порт занят")?;
    let port = listener.local_addr()?.port();
    let host = format!("127.0.0.1:{port}");
    let store = Arc::new(Store::open(&data.join("state.sqlite3"))?);
    store.reconcile_restart()?;
    if !store.has_any_projects()? {
        seed_demo(&data, &store)?;
    }
    let token = mpac_daemon::auth::load_or_create(&data)?;
    let url = format!("http://{host}/#token={token}");
    let app = Arc::new(App::new(store, token, host));
    let launch_file = data.join("launch.json");
    fs::write(
        &launch_file,
        serde_json::to_vec_pretty(
            &json!({"url":url,"pid":std::process::id(),"port":port,"prototype":true}),
        )?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&launch_file, fs::Permissions::from_mode(0o600))?;
    }
    println!(
        "MultiProjectAgentControl — локальный прототип\n{url}\nCtrl+C останавливает daemon и его сессии."
    );
    let shutdown_app = app.clone();
    let router = api::router(app.clone(), &ui);
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("signal handler");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            shutdown_app.stop_all();
        })
        .await?;
    app.stop_all();
    for _ in 0..100 {
        if app
            .live
            .lock()
            .unwrap()
            .values()
            .all(|s| !s.meta().active())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    let _ = fs::remove_file(launch_file);
    drop(lock);
    Ok(())
}

fn seed_demo(data: &Path, store: &Store) -> Result<()> {
    let mut roots = Vec::new();
    for name in ["backend", "frontend"] {
        let path = data.join("demo").join(name);
        fs::create_dir_all(&path)?;
        if !path.join(".git").exists() {
            git(&path, &["init", "--quiet"])?;
            fs::write(
                path.join("README.md"),
                format!(
                    "# {name}\n\nДемо-проект MultiProjectAgentControl.\n\n## Задачи\n\n- [x] Запустить сессию\n- [ ] Прочитать артефакт\n- [ ] Проверить изменения\n"
                ),
            )?;
            git(&path, &["add", "README.md"])?;
            git(
                &path,
                &[
                    "-c",
                    "user.name=MPAC Demo",
                    "-c",
                    "user.email=demo@localhost",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                    "commit",
                    "--quiet",
                    "-m",
                    "Initial demo",
                ],
            )?;
            fs::write(
                path.join("README.md"),
                format!(
                    "# {name}\n\nДемо-проект MultiProjectAgentControl.\n\n## Задачи\n\n- [x] Запустить сессию\n- [x] Прочитать артефакт\n- [ ] Проверить изменения\n\n## Новое изменение\n\nДобавлено описание локального мониторинга сессий.\n"
                ),
            )?;
            fs::write(
                path.join("plan.md"),
                "# План работы\n\nЭтот файл создан только в демонстрационной папке.\n\n| Шаг | Статус |\n| --- | --- |\n| Проекты и CLI | Готово для проверки |\n| Чтение Markdown | Попробуйте открыть README.md |\n| Ревью | Посмотрите вкладку Изменения |\n\n```rust\nfn main() { println!(\"Hello, agents!\"); }\n```\n",
            )?;
        }
        let path = path.canonicalize()?;
        roots.push(mpac_workspace::Root {
            id: model::id(),
            name: name.into(),
            path: path.to_string_lossy().into_owned(),
        });
    }
    store.project(&Project {
        id: model::id(),
        name: "Демо · два репозитория".into(),
        roots,
    })
}

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .current_dir(cwd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if !status.success() {
        bail!("Не удалось подготовить демонстрационный Git repo");
    }
    Ok(())
}

fn demo_agent() -> Result<()> {
    let mut tree_started = false;
    println!("\x1b[1;36mMPAC · локальный демо-агент\x1b[0m\r\n");
    println!("Это безопасный CLI-сценарий без модели. Проект не изменяется.\r");
    println!("Напишите задачу и нажмите Enter. Команды: help, status, done, exit.\r");
    demo_question()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        match line.trim() {
            "tree" if !tree_started => {
                let mut child = Command::new(std::env::current_exe()?)
                    .arg("--demo-branch")
                    .spawn()?;
                println!("[demo-child {}]\r", child.id());
                io::stdout().flush()?;
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                tree_started = true;
            }
            "exit" | "quit" => {
                println!("Сессия завершена.\r");
                break;
            }
            "done" => {
                println!(
                    "\x1b[32mГотово. Проверьте артефакты и изменения во вкладках проекта.\x1b[0m\r"
                );
                break;
            }
            "help" => println!(
                "Введите задачу, status, done или exit. Review вставляется как черновик; нажмите Enter для отправки.\r"
            ),
            "status" => println!("Демо работает локально. API-вызовов и расходов нет.\r"),
            "" => {}
            text => {
                println!("\r\nПринято: {text}\r\n");
                for (n, step) in [
                    "Просматриваю задачу",
                    "Сверяю доступные артефакты",
                    "Подготовлено к ревью",
                ]
                .iter()
                .enumerate()
                {
                    println!("  {}. {}\r", n + 1, step);
                    io::stdout().flush()?;
                    std::thread::sleep(Duration::from_millis(450));
                }
            }
        }
        demo_question()?;
    }
    Ok(())
}

fn demo_question() -> Result<()> {
    print!("\r\n\x1b[33mНужен ваш ввод\x1b[0m → \x1b]777;mpac-demo;question\x07");
    io::stdout().flush()?;
    Ok(())
}

#[cfg(test)]
mod option_tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn bundled_paths_and_help_never_resolve_the_build_checkout() {
        let parsed = daemon_options(
            args(&[
                "--data-dir",
                "/application data/MPAC",
                "--ui-dir",
                "/bundle resources/ui",
                "--port",
                "0",
            ]),
            || panic!("bundled paths must not inspect the source checkout"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            parsed,
            DaemonOptions {
                data: PathBuf::from("/application data/MPAC"),
                ui: PathBuf::from("/bundle resources/ui"),
                port: 0
            }
        );
        assert_eq!(
            daemon_options(args(&["--help"]), || panic!(
                "help must not inspect the source checkout"
            ))
            .unwrap(),
            None
        );
    }

    #[test]
    fn local_defaults_and_partial_overrides_keep_existing_behavior() {
        let defaults = daemon_options(Vec::new(), || Ok(PathBuf::from("/checkout")))
            .unwrap()
            .unwrap();
        assert_eq!(
            defaults,
            DaemonOptions {
                data: PathBuf::from("/checkout/.mpac"),
                ui: PathBuf::from("/checkout/ui/dist"),
                port: 4317
            }
        );
        let partial = daemon_options(args(&["--data-dir", "relative-data"]), || {
            Ok(PathBuf::from("/checkout"))
        })
        .unwrap()
        .unwrap();
        assert_eq!(partial.data, PathBuf::from("relative-data"));
        assert_eq!(partial.ui, PathBuf::from("/checkout/ui/dist"));
        assert!(
            daemon_options(args(&["--data-dir", "/data"]), || bail!(
                "checkout unavailable"
            ))
            .is_err()
        );
    }

    #[test]
    fn malformed_options_fail_before_resolving_defaults() {
        for values in [
            vec!["--unknown"],
            vec!["--data-dir"],
            vec!["--ui-dir"],
            vec!["--port"],
            vec!["--port", "65536"],
        ] {
            assert!(
                daemon_options(args(&values), || panic!(
                    "invalid options must not inspect the checkout"
                ))
                .is_err()
            );
        }
    }
}
