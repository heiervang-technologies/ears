mod cli;
mod ghost_preview;
mod owned_file;
mod post_transcribe;

use ghost_preview::{
    deliver_via_ghost, finish_continuous, ghost_continuous_state_file, proc_start_time,
    recording_owner, run_ghost_preview, spawn_ghost_preview, stop_ghost_preview, watch_ghost,
};

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands, CsAction, DeviceAction, VadAction};
use ears::audio;
use ears::Config;
use ears::{
    AudioFeedback, FileLock, KeyboardLayout, Notifications, ProcessManager, State as StateEnum,
    StateManager, TextInput, WhisperClient,
};
use std::time::Duration;
use url::Url;

/// Levels for `debug.log` when `RUST_LOG` does not say otherwise.
const DEFAULT_LOG_FILTER: &str = "info";

/// The log filter: `RUST_LOG` when set and valid, else [`DEFAULT_LOG_FILTER`].
/// ONNX Runtime narrates every model load at INFO, so `ort` stays at WARN
/// unless `RUST_LOG` names it (`RUST_LOG=info,ort=info`).
fn log_filter(rust_log: Option<&str>) -> tracing_subscriber::EnvFilter {
    let quiet_ort = |directives: &str| format!("ort=warn,{directives}");
    rust_log
        .filter(|directives| !directives.trim().is_empty())
        .and_then(|directives| tracing_subscriber::EnvFilter::try_new(quiet_ort(directives)).ok())
        .unwrap_or_else(|| tracing_subscriber::EnvFilter::new(quiet_ort(DEFAULT_LOG_FILTER)))
}

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI first (need profile for config loading)
    let cli = Cli::parse();

    // Headless VAD check: no config, devices or logging needed.
    if let Some(Commands::VadFile { file, threshold }) = &cli.command {
        let scan = ears::vad::scan_wav(file, *threshold)?;
        let speech = scan.spans.iter().fold(0.0f32, |s, (a, b, _)| s + (b - a));
        for (start, end, peak) in &scan.spans {
            println!("speech {start:.2}-{end:.2} s  peak {peak:.2}");
        }
        println!(
            "{:.2} s of speech in {:.2} s ({} span{})",
            speech,
            scan.seconds,
            scan.spans.len(),
            if scan.spans.len() == 1 { "" } else { "s" }
        );
        return Ok(());
    }

    // Runs on every shell prompt: answer before config loading and logging.
    if let Some(Commands::Prompt {
        state,
        recording,
        transcribing,
        vad,
        standby,
    }) = &cli.command
    {
        if let Some(current) = Config::state_dir_path()
            .ok()
            .and_then(|dir| ears::prompt::current(&dir))
        {
            if state.as_deref().is_none_or(|s| s == current.name()) {
                let text = match current {
                    ears::prompt::PromptState::Recording => recording,
                    ears::prompt::PromptState::Transcribing => transcribing,
                    ears::prompt::PromptState::Vad => vad,
                    ears::prompt::PromptState::Standby => standby,
                };
                println!("{text}");
            }
        }
        return Ok(());
    }

    // Polled by status bars and scripts: no config or logging, only a read.
    if let Some(Commands::Health { json }) = &cli.command {
        let report = ears::health_report::read(&Config::state_dir_path()?);
        if *json {
            println!("{}", serde_json::to_string(&report)?);
        } else {
            print!("{}", ears::health_report::render(&report));
        }
        std::process::exit(report.exit_code);
    }

    // Load config with profile
    let config = Config::load_profile_or_defaults(cli.profile.as_deref())?;

    // Set up debug logging to file
    let log_file_path = config.state_dir.join("debug.log");
    // Only the long-running owner caps the log, before opening it; the
    // short-lived subcommands polled by bars and scripts only append.
    let rotated = owns_debug_log(&cli.command, || vad_running(&config)).then(|| {
        ears::log_rotate::rotate_if_larger(&log_file_path, ears::log_rotate::ROTATE_BYTES)
    });
    if let Ok(log_file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_file_path)
    {
        let _ = tracing_subscriber::fmt()
            .with_writer(std::sync::Arc::new(log_file))
            .with_ansi(false)
            .with_env_filter(log_filter(std::env::var("RUST_LOG").ok().as_deref()))
            .try_init();
    }

    // Short-lived subcommands run constantly (a status bar polls `ears typing
    // status` every couple of seconds) and all append to the same file: only
    // the long-running modes announce themselves at INFO.
    if matches!(
        cli.command,
        None | Some(Commands::Vad { .. } | Commands::Ghost | Commands::WsListen { .. })
    ) {
        tracing::info!(pid = std::process::id(), "ears started");
    } else {
        tracing::debug!("ears started");
    }
    tracing::debug!("Log file: {}", log_file_path.display());
    match rotated {
        Some(Ok(true)) => tracing::info!(
            "Rotated debug.log above {} bytes to debug.log.1",
            ears::log_rotate::ROTATE_BYTES
        ),
        Some(Err(e)) => tracing::warn!("Could not rotate debug.log: {}", e),
        _ => {}
    }

    // Set audio cue volume from config
    AudioFeedback::set_volume(config.cue_volume);

    match cli.command {
        Some(Commands::Toggle { ghost }) => {
            handle_toggle(&config, ghost).await?;
        }
        Some(Commands::GhostPreview {
            recorder_pid,
            recorder_start,
        }) => {
            run_ghost_preview(&config, recorder_pid, recorder_start).await?;
        }
        Some(Commands::Vad { action }) => {
            if action == Some(VadAction::Status) {
                let running = vad_running(&config);
                println!("vad: {}", if running { "running" } else { "stopped" });
                std::process::exit(if running { 0 } else { 1 });
            }
            if config.vad.ghost && action != Some(VadAction::Stop) {
                ears::ghost_style::apply(&config.ghost);
            }
            handle_vad(&config, config.vad.ghost, action).await?;
        }
        Some(Commands::Ghost) => {
            ears::ghost_style::apply(&config.ghost);
            handle_vad(&config, true, None).await?;
        }
        Some(Commands::GhostWatch { json }) => {
            watch_ghost(json).await?;
        }
        Some(Commands::GhostStyle {
            color,
            underline,
            no_underline,
            frozen,
        }) => {
            ghost_style_command(config, color, underline, no_underline, frozen)?;
        }
        Some(Commands::WsListen {
            host,
            port,
            socket,
            keep_audio,
            live,
            say_echo,
        }) => {
            handle_ws_listen(&config, &host, port, socket, keep_audio, live, say_echo).await?;
        }
        Some(Commands::Device { action }) => match action {
            Some(DeviceAction::List) => list_devices()?,
            Some(DeviceAction::Select) => select_device(&config)?,
            Some(DeviceAction::Current) | None => show_current(&config)?,
        },
        Some(Commands::Profile { name }) => {
            if let Some(name) = name {
                set_profile(&name)?;
            } else {
                show_profile()?;
            }
        }
        Some(Commands::Server { url }) => {
            if let Some(url_str) = url {
                set_server(&config, &url_str)?;
            } else {
                show_server(&config)?;
            }
        }
        // answered before config loading, above
        Some(Commands::VadFile { .. }) => {}
        Some(Commands::ChantFile { file }) => {
            let state_dir = std::env::temp_dir().join("ears-chant-file");
            std::fs::create_dir_all(&state_dir)?;
            let mut mode =
                ears::command_mode::CommandMode::start(ears::command_mode::Paths::new(&state_dir))
                    .await?;
            let bytes = std::fs::read(file)?;
            let samples = ears::pcm::samples_f32(bytes.get(44..).unwrap_or_default());
            let language = config.language.clone();
            let (server_url, model) = config.resolve_server(language.as_deref());
            let whisper = std::sync::Arc::new(
                WhisperClient::new(server_url.to_string())
                    .with_language(language)
                    .with_api_key(config.api_key.clone())
                    .with_model(model)
                    .with_silence_filter(config.filter_silence_artifacts)
                    .with_prompt(config.prompt.clone()),
            );
            ears::chant::replay(&mut mode, whisper, &samples).await;
        }
        Some(Commands::Test { file }) => {
            test_config(&config, file.as_deref()).await?;
        }
        Some(Commands::MicCheck {
            device,
            no_transcribe,
            json,
        }) => {
            ears::mic_check::run(&config, device, !no_transcribe, json).await?;
        }
        Some(Commands::Prompt { .. }) => unreachable!("handled before config load"),
        Some(Commands::Health { .. }) => unreachable!("handled before config load"),
        Some(Commands::AutoEnter) => {
            handle_auto_enter().await?;
        }
        Some(Commands::Typing { action }) => {
            handle_typing(&action).await?;
        }
        Some(Commands::Standby { action }) => {
            handle_standby(&action, &config.state_dir).await?;
        }
        Some(Commands::CommandMode { action }) => {
            handle_command_mode(&action, &config.state_dir).await?;
        }
        Some(Commands::Colour { colour }) => {
            match ears::ipc::send_command(&format!("colour-{colour}")).await {
                Ok(resp) if resp.starts_with("colour:") => {}
                Ok(resp) => anyhow::bail!("Running ears cannot switch colour: {resp}"),
                Err(_) => anyhow::bail!("VAD is not running"),
            }
        }
        Some(Commands::Teach { action }) => {
            handle_teach(&config, action.as_deref())?;
        }
        Some(Commands::Decide {
            question,
            options,
            timeout,
            index,
            other,
        }) => {
            let code = handle_decide(&config, &question, &options, other, timeout, index).await?;
            std::process::exit(code);
        }
        Some(Commands::Look { words, yes, json }) => {
            // Vision mode replaces a look still running with SIGTERM: leave
            // no overlay or state behind (the guard in handle_look drops).
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
            let said = words.join(" ");
            let code = tokio::select! {
                code = handle_look(&config, &said, yes, json) => code?,
                _ = term.recv() => {
                    ears::vision::clear(&config.state_dir);
                    143
                }
            };
            std::process::exit(code);
        }
        Some(Commands::Subtitles { state }) => {
            handle_subtitles(&config, state.as_deref())?;
        }
        Some(Commands::Cs { action }) => {
            let code = handle_cs(&config, action).await?;
            std::process::exit(code);
        }
        // Hidden backwards-compat aliases
        Some(Commands::Select) => select_device(&config)?,
        Some(Commands::List) => list_devices()?,
        Some(Commands::Current) => show_current(&config)?,
        None => {
            // Default: Launch TUI
            return ears::tui::run(cli.profile.as_deref()).await;
        }
    }

    Ok(())
}

async fn handle_auto_enter() -> Result<()> {
    match ears::ipc::send_command("toggle-auto-enter").await {
        Ok(resp) => {
            eprintln!("{}", resp);
            Ok(())
        }
        Err(_) => {
            anyhow::bail!("No running ears instance found (is VAD or TUI running?)");
        }
    }
}

/// `ears typing on|off|toggle|status`: ask the running instance, or update the
/// persisted switch directly when no ears is running.
/// `ears cs ...`: Compensating Scaffolding. Returns the exit code.
async fn handle_cs(config: &ears::Config, action: CsAction) -> Result<i32> {
    match action {
        CsAction::Check { model, file } => ears::cs::check::run(config, model, &file).await,
        CsAction::Migrate {
            out,
            config: config_path,
            actions,
            model,
        } => {
            let config_path = config_path.unwrap_or_else(|| {
                config
                    .config_dir
                    .join(match config.active_profile.as_deref() {
                        Some(profile) => format!("config.{profile}.toml"),
                        None => "config.toml".into(),
                    })
            });
            let actions = actions
                .unwrap_or_else(|| ears::command_mode::Paths::new(&config.state_dir).actions);
            ears::cs::migrate::run(&config_path, &actions, &model, out.as_deref())?;
            Ok(0)
        }
        CsAction::Mine {
            since,
            out,
            journal,
            model,
        } => {
            let args = ears::cs::mine::MineArgs {
                since,
                out,
                journal,
                model,
            };
            ears::cs::mine::run(config, &args)?;
            Ok(0)
        }
        CsAction::Build(args) => {
            ears::cs::probe::build(config, &args).await?;
            Ok(0)
        }
    }
}

/// `ears teach [ACTION]`.
fn handle_teach(config: &ears::Config, action: Option<&str>) -> Result<()> {
    use ears::command_mode as cm;
    let paths = cm::Paths::new(&config.state_dir);
    let (_, actions) = cm::load_actions(&paths.actions)?;
    let voice = cm::VoiceAnchors::load(&paths.voice);
    let Some(action) = action else {
        println!("Actions ({}):", paths.actions.display());
        for a in &actions {
            let recorded = voice.anchors.iter().filter(|v| v.action == a.name).count();
            println!("  {:16} {} text, {} voice", a.name, a.text.len(), recorded);
        }
        return Ok(());
    };
    if !actions.iter().any(|a| a.name == action) {
        anyhow::bail!("no action named {action:?}; `ears teach` lists them");
    }
    std::fs::create_dir_all(&config.state_dir)?;
    std::fs::write(config.state_dir.join(cm::TEACH), action)?;
    let on = config.state_dir.join(cm::MARKER).exists();
    println!(
        "Say what you would say for {action}{}",
        if on {
            "."
        } else {
            " once command mode is on (say \"command\")."
        }
    );
    Ok(())
}

/// `ears look`: one round of vision mode (docs/VISION_MODE.md).
/// Act at `target` only if the screen is still the one SAM saw (same
/// output, workspace and focused window): otherwise the point may now sit
/// on something else, and a click there is the one thing not to do.
fn act_on_same_screen(
    compositor: ears::pointer::Compositor,
    output: &ears::pointer::Output,
    target: &ears::vision::Shown,
    action: ears::vision::Action,
    scene: &Option<String>,
) -> Result<()> {
    let now = ears::pointer::scene(compositor);
    if scene.is_some() && now != *scene {
        ears::desktop::AudioFeedback::beep_error().ok();
        anyhow::bail!("the screen changed since the screenshot ({scene:?} -> {now:?}): not acting");
    }
    ears::vision::act(compositor, output, target, action)
}

async fn handle_look(config: &ears::Config, said: &str, yes: bool, json: bool) -> Result<i32> {
    use ears::pointer::{self, Compositor};
    use ears::vision::{self, Action, ButtonName, Request, State};

    let mut req = Request::parse(said);
    if req.target.is_empty() {
        anyhow::bail!("nothing to look for in {said:?}");
    }
    let compositor = Compositor::detect()?;
    let output = pointer::focused_output(compositor)?;
    let dir = &config.state_dir;
    let server = &config.vision.server;
    let image = vision::image_path(dir);
    let id = format!("{}", std::process::id());
    let state = |s: &str, hint: String, shown: Vec<vision::Shown>, picked: usize| State {
        id: id.clone(),
        state: s.to_string(),
        prompt: String::new(),
        output: output.name.clone(),
        width: output.width,
        height: output.height,
        image: if shown.is_empty() {
            String::new()
        } else {
            image.display().to_string()
        },
        picked,
        candidates: shown,
        hint,
    };
    // Gone with the process, whatever path it leaves by.
    struct Clear<'a>(&'a std::path::Path);
    impl Drop for Clear<'_> {
        fn drop(&mut self) {
            ears::vision::clear(self.0);
        }
    }
    let _clear = Clear(dir);

    let t0 = std::time::Instant::now();
    let mut jpeg = pointer::screenshot(&output)?;
    let mut scene = pointer::scene(compositor);
    let shot_ms = t0.elapsed().as_millis();
    vision::write_state(
        dir,
        &state("looking", format!("looking for {}", req.target), vec![], 0),
    )?;
    let t1 = std::time::Instant::now();
    let (found, mut size) = vision::segment(server, &jpeg, &req.target).await?;
    let mut cands = vision::rank(found, req.which);
    tracing::info!(
        "look {:?}: screenshot {} ms, SAM {} ms, {} candidates",
        req.target,
        shot_ms,
        t1.elapsed().as_millis(),
        cands.len()
    );
    if json {
        let shown = vision::shown(&cands, size, &output, &req.target);
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(0);
    }
    let mut picked = 0usize;
    let mut action = req.action;
    let vad = vad_running(config);
    loop {
        if cands.is_empty() {
            vision::write_state(
                dir,
                &state("none", format!("no {} found", req.target), vec![], 0),
            )?;
            eprintln!("No {} found", req.target);
            tokio::time::sleep(Duration::from_millis(1500)).await;
            return Ok(1);
        }
        picked = picked.min(cands.len() - 1);
        vision::render(&cands, picked, &image)?;
        let shown = vision::shown(&cands, size, &output, &req.target);
        let target = &shown[picked];
        if yes && action != Action::Outline {
            vision::write_state(
                dir,
                &state("acting", req.target.clone(), shown.clone(), picked),
            )?;
            act_on_same_screen(compositor, &output, target, action, &scene)?;
            return Ok(0);
        }
        let hint = format!("{} {}/{}", req.target, picked + 1, shown.len());
        vision::write_state(dir, &state("pick", hint.clone(), shown.clone(), picked))?;
        if !vad {
            for s in &shown {
                println!(
                    "[{}] score={:.2} at {:.0},{:.0}",
                    s.n, s.score, s.point[0], s.point[1]
                );
            }
            eprintln!(
                "ears VAD is not running: nobody would hear the answer (use --yes with a verb)"
            );
            tokio::time::sleep(Duration::from_secs(4)).await;
            return Ok(0);
        }
        // A verb said up front still waits for a yes.
        let mut options: Vec<String> = ["click", "right click", "middle click", "next"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        if let Action::Click(_) | Action::DoubleClick | Action::Move = action {
            options.insert(0, "yes".to_string());
        }
        let timeout = Some(Duration::from_secs(config.vision.confirm_s));
        let answer = ears::decide::ask(dir, &hint, &options, true, true, timeout).await?;
        let Some(answer) = answer else {
            return Ok(2);
        };
        let chosen = answer.choice.as_deref();
        let custom = answer.custom.as_deref().unwrap_or("");
        let act = |a: Action| act_on_same_screen(compositor, &output, target, a, &scene);
        match chosen {
            Some("yes") => {
                act(action)?;
                return Ok(0);
            }
            Some("click") => {
                act(Action::Click(ButtonName::Left))?;
                return Ok(0);
            }
            Some("right click") => {
                act(Action::Click(ButtonName::Right))?;
                return Ok(0);
            }
            Some("middle click") => {
                act(Action::Click(ButtonName::Middle))?;
                return Ok(0);
            }
            Some("next") => {
                picked = (picked + 1) % cands.len();
                continue;
            }
            Some(_) => {}
            None if custom.is_empty() => return Ok(1), // cancelled
            None => {}
        }
        // An answer of its own.
        if let Some(n) = vision::parse_scroll(custom) {
            act(Action::Scroll(n))?;
            // The page moved: look again for the same thing.
            tokio::time::sleep(Duration::from_millis(250)).await;
            jpeg = pointer::screenshot(&output)?;
            scene = pointer::scene(compositor);
            let (found, s) = vision::segment(server, &jpeg, &req.target).await?;
            cands = vision::rank(found, req.which);
            size = s;
            picked = 0;
            continue;
        }
        let said = Request::parse(custom);
        let only_number = custom
            .split_whitespace()
            .filter_map(vision::number)
            .next()
            .filter(|_| custom.split_whitespace().count() <= 2);
        if let Some(n) = only_number.filter(|n| *n >= 1 && (*n as usize) <= cands.len()) {
            picked = n as usize - 1;
            continue;
        }
        if said.target.is_empty() && said.action != Action::Outline {
            act(said.action)?;
            return Ok(0);
        }
        if !said.target.is_empty() {
            // Something else on the same screen.
            if said.action != Action::Outline {
                action = said.action;
            }
            req = said;
            let (found, s) = vision::segment(server, &jpeg, &req.target).await?;
            cands = vision::rank(found, req.which);
            size = s;
            picked = 0;
        }
    }
}

async fn handle_decide(
    config: &ears::Config,
    question: &str,
    options: &[String],
    other: bool,
    timeout: Option<u64>,
    index: bool,
) -> Result<i32> {
    let vad = std::fs::read_to_string(config.state_dir.join("vad.pid"))
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<i32>().ok())
        // SAFETY: signal 0 only checks that the process exists.
        .is_some_and(|pid| pid > 0 && unsafe { libc::kill(pid, 0) == 0 });
    if !vad {
        eprintln!("ears VAD is not running: nobody would hear the answer");
        return Ok(3);
    }
    // A timeout needs both: the asker's, and the config allowing them.
    let timeout = timeout
        .filter(|_| config.decide_timeouts)
        .map(std::time::Duration::from_secs);
    let answer =
        ears::decide::ask(&config.state_dir, question, options, other, false, timeout).await?;
    Ok(match answer {
        None => {
            eprintln!("No answer");
            2
        }
        Some(ears::decide::Answer {
            choice: Some(choice),
            index: Some(i),
            ..
        }) => {
            if index {
                println!("{i}");
            } else {
                println!("{choice}");
            }
            0
        }
        Some(ears::decide::Answer {
            custom: Some(custom),
            ..
        }) => {
            println!("{}", if index { "other" } else { custom.as_str() });
            0
        }
        Some(_) => {
            eprintln!("Cancelled");
            1
        }
    })
}

fn handle_subtitles(config: &ears::Config, state: Option<&str>) -> Result<()> {
    let on = match state {
        None => config.state_dir.join(ears::subtitles::MARKER).exists(),
        Some("on") => true,
        Some("off") => false,
        Some(other) => anyhow::bail!("expected on or off, not {other:?}"),
    };
    if state.is_some() {
        ears::subtitles::set(&config.state_dir, on, &config.subtitle_modes)?;
    }
    println!("Subtitles {}", if on { "on" } else { "off" });
    Ok(())
}

async fn handle_typing(action: &str) -> Result<()> {
    use ears::typing_switch::{self, TypingRequest};
    let request = TypingRequest::from_arg(action)
        .with_context(|| format!("Unknown typing action: {}", action))?;
    match ears::ipc::send_command(request.command()).await {
        Ok(resp) if resp.starts_with("typing:") => {
            println!("{}", resp);
            Ok(())
        }
        Ok(resp) => anyhow::bail!("Running ears does not support typing control: {}", resp),
        Err(_) => {
            let enabled = request.apply(typing_switch::load());
            if request.changes_state() {
                typing_switch::save(enabled)?;
            }
            println!("{}", typing_switch::describe(enabled));
            Ok(())
        }
    }
}

/// `ears standby on|off|toggle|status`: on stands VAD by (only the wake
/// phrase is acted on), off resumes it, as the spoken phrases do. Status reads
/// the marker VAD keeps while in standby.
async fn handle_standby(action: &str, state_dir: &std::path::Path) -> Result<()> {
    let marker = state_dir.join(ears::prompt::STANDBY_MARKER);
    let status = || {
        println!("standby:{}", if marker.exists() { "on" } else { "off" });
    };
    match action {
        "status" => {
            status();
            Ok(())
        }
        "on" | "off" | "toggle" => match ears::ipc::send_command(&format!("standby-{}", action))
            .await
        {
            Ok(resp) if resp.starts_with("standby:") => {
                // the engine applies it on its next turn; give it a moment so status is current
                for _ in 0..20 {
                    let on = marker.exists();
                    if (action == "on" && on) || (action == "off" && !on) || action == "toggle" {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                status();
                Ok(())
            }
            Ok(resp) => anyhow::bail!("Running ears does not support standby control: {}", resp),
            Err(_) => anyhow::bail!("VAD is not running"),
        },
        other => anyhow::bail!("Unknown standby action: {}", other),
    }
}

/// `ears command on|off|toggle|status`: command mode, as saying "command"
/// switches it. Status reads the marker VAD keeps while it is on.
async fn handle_command_mode(action: &str, state_dir: &std::path::Path) -> Result<()> {
    let marker = state_dir.join(ears::command_mode::MARKER);
    let status = || {
        println!("command:{}", if marker.exists() { "on" } else { "off" });
    };
    match action {
        "status" => {
            status();
            Ok(())
        }
        "on" | "off" | "toggle" => match ears::ipc::send_command(&format!("command-{}", action))
            .await
        {
            Ok(resp) if resp.starts_with("command:") => {
                // entering loads the actions: give it a moment so status is current
                for _ in 0..60 {
                    let on = marker.exists();
                    if (action == "on" && on) || (action == "off" && !on) || action == "toggle" {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                status();
                Ok(())
            }
            Ok(resp) => anyhow::bail!(
                "Running ears does not support command mode control: {}",
                resp
            ),
            Err(_) => anyhow::bail!("VAD is not running"),
        },
        other => anyhow::bail!("Unknown command mode action: {}", other),
    }
}

fn select_device(config: &Config) -> Result<()> {
    let devices = audio::list_devices().context("Failed to list audio devices")?;

    if devices.is_empty() {
        eprintln!("No audio input devices found");
        anyhow::bail!("No audio input devices available");
    }

    let selected =
        audio::select_device_interactive(&devices).context("Failed to run device selection")?;

    let device_name = match selected {
        Some(name) => name,
        None => {
            eprintln!("No device selected");
            return Ok(());
        }
    };

    let device = devices
        .iter()
        .find(|d| d.name == device_name)
        .context("Selected device not found in list")?;

    let mut config = config.clone();
    config.device = device_name.clone();
    config.save().context("Failed to save configuration")?;

    println!("Selected: {}", device.description);
    println!("Device ID: {}", device_name);
    println!("Saved to: {}", config.config_file().display());

    Ok(())
}

fn list_devices() -> Result<()> {
    let devices = audio::list_devices().context("Failed to list audio devices")?;

    if devices.is_empty() {
        println!("No audio input devices found");
        return Ok(());
    }

    let formatted = audio::format_device_list(&devices);

    use std::io::Write;
    use std::process::{Command, Stdio};

    let child = Command::new("column")
        .arg("-t")
        .arg("-s")
        .arg("\t")
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    match child {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(formatted.as_bytes());
            }
            let _ = child.wait();
        }
        Err(_) => {
            println!("{}", formatted);
        }
    }

    Ok(())
}

fn show_current(config: &Config) -> Result<()> {
    println!("Current device: {}", config.device);
    println!("Config: {}", config.config_file().display());

    Ok(())
}

fn set_server(config: &Config, url_str: &str) -> Result<()> {
    let url = Url::parse(url_str).with_context(|| format!("Invalid server URL: {}", url_str))?;

    if !matches!(url.scheme(), "http" | "https") {
        anyhow::bail!(
            "Invalid URL scheme: {} (must be http or https)",
            url.scheme()
        );
    }

    if url.host().is_none() {
        anyhow::bail!("Server URL must have a host");
    }

    let mut config = config.clone();
    config.whisper_server = url;
    config.save().context("Failed to save configuration")?;

    println!("Server set to: {}", config.whisper_server);

    Ok(())
}

fn show_profile() -> Result<()> {
    let current = Config::get_default_profile()?;
    match current {
        Some(ref name) => println!("Default profile: {}", name),
        None => println!("No default profile set (using config.toml)"),
    }

    let profiles = Config::list_profiles()?;
    if profiles.is_empty() {
        println!("\nNo named profiles found.");
        println!("Create one at ~/.config/ears/config.<name>.toml");
    } else {
        println!("\nAvailable profiles:");
        for p in &profiles {
            let marker = if current.as_deref() == Some(p) {
                " *"
            } else {
                ""
            };
            println!("  {}{}", p, marker);
        }
    }

    Ok(())
}

fn set_profile(name: &str) -> Result<()> {
    if name == "default" || name.is_empty() {
        Config::set_default_profile("")?;
        println!("Profile cleared (using config.toml)");
    } else {
        Config::set_default_profile(name)?;
        println!("Default profile set to: {}", name);
    }
    Ok(())
}

fn show_server(config: &Config) -> Result<()> {
    println!("Current server: {}", config.whisper_server);
    println!("Config: {}", config.config_file().display());

    Ok(())
}

/// Mask a secret for display: keep a short prefix/suffix, hide the middle.
fn mask_secret(secret: &str) -> String {
    let len = secret.chars().count();
    if len <= 8 {
        return "set (hidden)".to_string();
    }
    let prefix: String = secret.chars().take(4).collect();
    let suffix: String = secret.chars().skip(len - 3).collect();
    format!("set ({}…{})", prefix, suffix)
}

/// Validate the active configuration: print a summary, check server health, and
/// optionally transcribe a sample audio file. Exits non-zero on failure.
async fn test_config(config: &Config, file: Option<&str>) -> Result<()> {
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());

    println!("Config:   {}", config.config_file().display());
    println!(
        "Profile:  {}",
        config.active_profile.as_deref().unwrap_or("default")
    );
    println!("Server:   {}", server_url);
    // Mirror the endpoint WhisperClient actually builds (trim trailing '/').
    println!(
        "Endpoint: {}/v1/audio/transcriptions",
        server_url.as_str().trim_end_matches('/')
    );
    println!(
        "Model:    {}",
        model.as_deref().unwrap_or("(server default)")
    );
    println!("Device:   {}", config.device);
    println!(
        "Language: {}",
        language.as_deref().unwrap_or("(auto-detect)")
    );
    println!(
        "API key:  {}",
        config
            .api_key
            .as_deref()
            .map(mask_secret)
            .unwrap_or_else(|| "(none)".to_string())
    );

    if config.server_has_redundant_v1() {
        println!();
        println!(
            "⚠  Server URL ends in /v1. ears appends /v1/audio/transcriptions itself, so\n   \
             requests hit a doubled /v1/v1 path and will fail. Drop the trailing /v1."
        );
    }

    println!();
    print!("Health check… ");
    use std::io::Write;
    std::io::stdout().flush().ok();

    let client = WhisperClient::new(server_url.to_string())
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(model)
        .with_silence_filter(config.filter_silence_artifacts)
        .with_prompt(config.prompt.clone());

    if let Err(e) = client.health_check().await {
        println!("FAILED");
        anyhow::bail!("Server health check failed: {}", e);
    }
    println!("OK");

    if let Some(path) = file {
        print!("Transcribing {}… ", path);
        std::io::stdout().flush().ok();
        match client.transcribe(path).await {
            Ok(text) => {
                println!("OK");
                println!("Transcript: {}", text);
            }
            Err(e) => {
                println!("FAILED");
                anyhow::bail!("Transcription failed: {}", e);
            }
        }
    }

    println!();
    println!("✓ Config OK");
    Ok(())
}

// Use shared StateResetGuard from ears::state

/// Toggle VAD mode: start or stop headless voice activity detection
/// A VAD process is alive (its pid file names one).
/// Whether this invocation is a long-running mode that owns `debug.log`
/// rotation: the TUI, `ws-listen`, and `vad`/`ghost` when they start VAD
/// (a toggle that stops a running VAD exits at once and must not rotate).
fn owns_debug_log(command: &Option<Commands>, vad_running: impl FnOnce() -> bool) -> bool {
    match command {
        None | Some(Commands::WsListen { .. }) => true,
        Some(Commands::Vad {
            action: None | Some(VadAction::Start),
        })
        | Some(Commands::Ghost) => !vad_running(),
        _ => false,
    }
}

fn vad_running(config: &Config) -> bool {
    ears::health_report::pid_file_alive(&config.state_dir.join("vad.pid"))
}

/// Start, stop or toggle (`action` None) VAD.
async fn handle_vad(config: &Config, ghost: bool, action: Option<VadAction>) -> Result<()> {
    AudioFeedback::set_volume(config.cue_volume);
    let vad_pid_file = config.state_dir.join("vad.pid");
    let vad_lock_path = config.state_dir.join("vad.lock");
    match action {
        Some(VadAction::Start) if vad_running(config) => {
            eprintln!("VAD already running");
            return Ok(());
        }
        Some(VadAction::Stop) if !vad_running(config) => {
            eprintln!("VAD not running");
            return Ok(());
        }
        _ => {}
    }
    let mut vad_lock = FileLock::new(&vad_lock_path).context("Failed to create VAD lock")?;
    if !vad_lock.try_lock().context("Failed to check VAD lock")? {
        if action == Some(VadAction::Start) {
            // Another VAD is starting (or restarting itself): leave it be.
            eprintln!("VAD already starting");
            return Ok(());
        }
        // Another VAD instance is starting up — just try to kill it via PID
        tracing::info!("VAD lock held by another process, attempting stop via PID");
    }

    if let Ok(pid_str) = std::fs::read_to_string(&vad_pid_file) {
        if let Ok(pid) = pid_str.trim().parse::<i32>() {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;

            if kill(Pid::from_raw(pid), None).is_ok() {
                if let Err(e) = kill(Pid::from_raw(pid), Signal::SIGTERM) {
                    tracing::warn!("Failed to send SIGTERM to VAD process {}: {}", pid, e);
                }
                AudioFeedback::beep_vad_close().ok();
                eprintln!("VAD stopped");
                return Ok(());
            }
        }
        if let Err(e) = std::fs::remove_file(&vad_pid_file) {
            tracing::debug!("Failed to remove stale VAD PID file: {}", e);
        }
    }

    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;
    if let Err(e) = state_mgr.load_state() {
        tracing::warn!("Failed to load state: {}", e);
    }

    std::fs::write(&vad_pid_file, std::process::id().to_string())
        .context("Failed to write VAD PID file")?;

    if state_mgr.current_state() != StateEnum::Idle {
        if let Err(e) = state_mgr.transition(StateEnum::Idle) {
            tracing::warn!("Failed to reset state to Idle: {}", e);
        }
    }
    state_mgr
        .transition(StateEnum::VadActive)
        .context("Failed to transition to VadActive")?;

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ears::streaming_engine::StreamingEvent>();

    let (shutdown_tx, settings_tx, pipeline_handle) =
        match ears::tui::start_vad_pipeline(config, event_tx).await {
            Ok(result) => result,
            Err(e) => {
                if let Err(e2) = std::fs::remove_file(&vad_pid_file) {
                    tracing::debug!("Failed to remove VAD PID file: {}", e2);
                }
                if let Err(e2) = state_mgr.transition(StateEnum::Idle) {
                    tracing::warn!("Failed to reset state after VAD failure: {}", e2);
                }
                anyhow::bail!("Failed to start VAD: {}", e);
            }
        };

    // Helper to build typing settings from current config state
    // With typing off, the engine only transcribes and publishes on ears.sock.
    // A held claim switches typing off the same way, without the cue or the
    // persisted switch, and keeps partials coming for the claimant.
    let make_settings = |auto_enter: bool, typing: bool, claimed: bool| {
        let typing = typing && !claimed;
        let language = ears::KeyboardLayout::detect_language().or_else(|| config.language.clone());
        ears::tui::TypingSettings {
            progressive_typing: typing,
            auto_correction: true,
            typing_mode: ears::typing_switch::effective_mode(config.typing_mode, typing),
            auto_enter: auto_enter && typing,
            text_filters: config.text_filters.clone(),
            language,
            guided_grammar: config.active_grammar(),
            ghost,
            claimed,
            wake: None,
            command: None,
            colour: None,
        }
    };

    // Send config-driven settings to the engine
    let mut typing = ears::typing_switch::load();
    let _ = settings_tx.send(make_settings(config.auto_enter, typing, false));

    AudioFeedback::beep_vad_open().ok();
    if ghost {
        eprintln!("VAD started (ghost completion) - listening...");
    } else {
        eprintln!("VAD started - listening...");
    }

    let (ipc_tx, ipc_rx) = tokio::sync::broadcast::channel(100);
    let ipc_server = ears::ipc::start_ipc_server(ipc_rx);

    // Start command server for remote control (e.g. `ears auto-enter`)
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let cmd_server = ears::ipc::start_cmd_server(cmd_tx);

    // Track mutable settings for command updates
    let mut auto_enter = config.auto_enter;
    let mut claims: usize = 0;
    let mut wake_seq: u64 = 0;
    let mut command_seq: u64 = 0;
    let mut colour_seq: u64 = 0;

    let save_to_clipboard = config.save_to_clipboard;
    let ducker = ears::ducker::VolumeDucker::new(config.vad.duck_enabled, config.vad.duck_percent);
    let event_ducker = ducker.clone();
    // Raised when capture dies underneath us so the main loop can exit and
    // leave a truthful (Idle) state instead of a stale VadActive.
    let capture_dead = std::sync::Arc::new(tokio::sync::Notify::new());
    let capture_dead_tx = capture_dead.clone();
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let _ = ipc_tx.send(event.clone());
            match event {
                ears::streaming_engine::StreamingEvent::SpeechProbable => {
                    AudioFeedback::beep_vad_speech_start().ok();
                    event_ducker.on_speech_probable();
                }
                ears::streaming_engine::StreamingEvent::SpeechStarted => {
                    AudioFeedback::beep_vad_speech_confirm().ok();
                }
                ears::streaming_engine::StreamingEvent::SpeechEnded => {
                    AudioFeedback::beep_vad_end().ok();
                    event_ducker.on_speech_ended();
                }
                ears::streaming_engine::StreamingEvent::SpeechRejected => {
                    event_ducker.on_speech_rejected();
                }
                ears::streaming_engine::StreamingEvent::SegmentDiscarded { reason } => {
                    tracing::info!("Nothing typed: {}", reason);
                    AudioFeedback::beep_vad_discard().ok();
                }
                ears::streaming_engine::StreamingEvent::CaptureStopped { reason } => {
                    tracing::warn!("Microphone capture stopped: {}", reason);
                    eprintln!("Microphone capture stopped: {}", reason);
                    event_ducker.on_speech_ended();
                    capture_dead_tx.notify_one();
                }
                ears::streaming_engine::StreamingEvent::SegmentCompleted {
                    text,
                    duration_ms,
                    ..
                } => {
                    tracing::info!("Segment: \"{}\" ({}ms)", text, duration_ms);
                    if save_to_clipboard && !text.is_empty() {
                        TextInput::copy_to_clipboard(&text);
                    }
                }
                ears::streaming_engine::StreamingEvent::Error(msg) => {
                    tracing::warn!("Streaming error: {}", msg);
                }
                _ => {}
            }
        }
    });

    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    // A lost capture (WirePlumber restarted, the device came back) starts
    // VAD again from scratch, unless it keeps failing (#220).
    let mut restart_capture = false;
    let mut gave_up = false;
    loop {
        tokio::select! {
            _ = sigterm.recv() => break,
            _ = sigint.recv() => break,
            _ = capture_dead.notified() => {
                let Some(delay) = capture_restart_delay(
                    std::env::var(CAPTURE_RESTARTS_ENV).ok().as_deref(),
                    unix_now(),
                ) else {
                    AudioFeedback::beep_vad_close().ok();
                    tracing::error!("VAD stopped: no audio input (it kept failing)");
                    gave_up = true;
                    break;
                };
                eprintln!("Microphone capture lost: listening again in {:?}", delay);
                tracing::warn!("Capture lost; restarting VAD in {:?}", delay);
                // Wait while still owning the pid file, so `ears vad` meanwhile
                // stops this one rather than starting a second.
                tokio::select! {
                    _ = tokio::time::sleep(delay) => restart_capture = true,
                    _ = sigterm.recv() => {}
                    _ = sigint.recv() => {}
                }
                break;
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    ears::ipc::EarsCommand::ToggleAutoEnter { respond } => {
                        auto_enter = !auto_enter;
                        let _ = settings_tx.send(make_settings(auto_enter, typing, claims > 0));
                        if auto_enter {
                            AudioFeedback::beep_toggle_on().ok();
                        } else {
                            AudioFeedback::beep_toggle_off().ok();
                        }
                        let state = if auto_enter { "on" } else { "off" };
                        let _ = respond.send(format!("auto-enter:{}", state));
                        tracing::info!("Auto-enter toggled to {}", auto_enter);
                        eprintln!("Auto-enter: {}", state);
                    }
                    ears::ipc::EarsCommand::Claim { held } => {
                        let before = claims > 0;
                        claims = if held { claims + 1 } else { claims.saturating_sub(1) };
                        if before != (claims > 0) {
                            let _ = settings_tx.send(make_settings(auto_enter, typing, claims > 0));
                            tracing::info!(
                                "Input {}",
                                if claims > 0 { "claimed: publishing instead of typing" } else { "released: typing again" }
                            );
                        }
                    }
                    ears::ipc::EarsCommand::CommandMode { on } => {
                        command_seq += 1;
                        let mut settings = make_settings(auto_enter, typing, claims > 0);
                        settings.command = Some((command_seq, on));
                        let _ = settings_tx.send(settings);
                        tracing::info!("Command mode switch asked for over IPC: {:?}", on);
                    }
                    ears::ipc::EarsCommand::Colour { colour } => {
                        colour_seq += 1;
                        let mut settings = make_settings(auto_enter, typing, claims > 0);
                        settings.colour = Some((colour_seq, colour));
                        let _ = settings_tx.send(settings);
                        tracing::info!("Colour switch asked for over IPC: {:?}", colour);
                    }
                    ears::ipc::EarsCommand::Standby { wake } => {
                        wake_seq += 1;
                        let mut settings = make_settings(auto_enter, typing, claims > 0);
                        settings.wake = Some((wake_seq, wake));
                        let _ = settings_tx.send(settings);
                        tracing::info!("Standby switch asked for over IPC: {:?}", wake);
                    }
                    ears::ipc::EarsCommand::Typing { request, respond } => {
                        let next = request.apply(typing);
                        if request.changes_state() {
                            if let Err(e) = ears::typing_switch::save(next) {
                                tracing::warn!("Failed to persist typing switch: {}", e);
                            }
                            if next != typing {
                                typing = next;
                                let _ = settings_tx.send(make_settings(auto_enter, typing, claims > 0));
                                if typing {
                                    AudioFeedback::beep_toggle_on().ok();
                                } else {
                                    AudioFeedback::beep_toggle_off().ok();
                                }
                                tracing::info!("Typing switched {}", if typing { "on" } else { "off" });
                            }
                        }
                        let _ = respond.send(ears::typing_switch::describe(typing));
                    }
                }
            }
        }
    }

    let _ = shutdown_tx.send(true);
    ears::pipeline::join_stopped(pipeline_handle).await;
    // Restore audio volume if we shut down mid-speech.
    ducker.on_speech_ended();
    drop(ducker);
    ipc_server.shutdown().await;
    cmd_server.shutdown().await;
    if let Err(e) = std::fs::remove_file(&vad_pid_file) {
        tracing::debug!("Failed to remove VAD PID file: {}", e);
    }
    if let Err(e) = state_mgr.transition(StateEnum::Idle) {
        tracing::warn!("Failed to transition to Idle on shutdown: {}", e);
    }
    if restart_capture {
        drop(vad_lock);
        restart_self();
        // Only reached when the exec failed.
        gave_up = true;
    }

    vad_exit(gave_up)
}

/// A VAD that was stopped ends cleanly; one that lost its microphone for good
/// fails, so that a supervisor (`contrib/systemd/ears-vad.service`) starts it
/// again rather than reading it as stopped on purpose.
fn vad_exit(gave_up: bool) -> Result<()> {
    if gave_up {
        anyhow::bail!("VAD stopped: no audio input (it kept failing)");
    }
    Ok(())
}

/// Carries "restarts in a row, last at (unix s)" across a capture restart.
const CAPTURE_RESTARTS_ENV: &str = "EARS_CAPTURE_RESTARTS";
/// Restarts within this many seconds of the last count as in a row.
const CAPTURE_RESTART_WINDOW_S: u64 = 120;
/// In a row, VAD gives up after this many (the device is really gone).
const CAPTURE_RESTARTS_MAX: u32 = 8;

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// How long to wait before restarting after a lost capture, given the
/// carried-over count (`"n,at"`); None to give up. Sets the count for the
/// restarted process.
fn capture_restart_delay(carried: Option<&str>, now: u64) -> Option<Duration> {
    let (count, at) = carried
        .and_then(|c| c.split_once(','))
        .and_then(|(n, at)| Some((n.parse::<u32>().ok()?, at.parse::<u64>().ok()?)))
        .unwrap_or((0, 0));
    let count = if now.saturating_sub(at) <= CAPTURE_RESTART_WINDOW_S {
        count + 1
    } else {
        1
    };
    if count > CAPTURE_RESTARTS_MAX {
        return None;
    }
    std::env::set_var(CAPTURE_RESTARTS_ENV, format!("{count},{now}"));
    Some(Duration::from_secs((1u64 << count).min(30)))
}

/// Replace this process with a fresh `ears` with the same arguments.
fn restart_self() {
    use std::os::unix::process::CommandExt;
    let mut args = std::env::args_os();
    let argv0 = args.next().unwrap_or_else(|| "ears".into());
    // A reinstalled binary leaves current_exe() pointing at "(deleted)".
    let program = std::env::current_exe()
        .ok()
        .filter(|p| p.exists())
        .map(std::ffi::OsString::from)
        .unwrap_or(argv0);
    let err = std::process::Command::new(&program).args(args).exec();
    AudioFeedback::beep_vad_close().ok();
    eprintln!("VAD stopped: cannot restart {:?}: {}", program, err);
}

#[cfg(test)]
mod capture_restart_tests {
    use super::*;

    #[test]
    fn giving_up_on_the_microphone_is_a_failure_and_a_stop_is_not() {
        assert!(vad_exit(false).is_ok());
        assert!(vad_exit(true).is_err());
    }

    #[test]
    fn backs_off_in_a_row_and_resets_after_a_quiet_spell() {
        assert_eq!(
            capture_restart_delay(None, 1000),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            capture_restart_delay(Some("2,990"), 1000),
            Some(Duration::from_secs(8))
        );
        assert_eq!(
            capture_restart_delay(Some("6,990"), 1000),
            Some(Duration::from_secs(30))
        );
        assert_eq!(capture_restart_delay(Some("8,990"), 1000), None);
        assert_eq!(
            capture_restart_delay(Some("8,100"), 1000),
            Some(Duration::from_secs(2)),
            "an old run of failures is forgotten"
        );
        assert_eq!(
            capture_restart_delay(Some("junk"), 1000),
            Some(Duration::from_secs(2))
        );
    }
}

/// Run WebSocket audio input mode: starts a WS server that feeds audio into the VAD pipeline
async fn handle_ws_listen(
    config: &Config,
    host: &str,
    port: u16,
    socket: Option<String>,
    keep_audio: Option<std::path::PathBuf>,
    live: bool,
    say_echo: bool,
) -> Result<()> {
    // Use a custom socket path to avoid conflicting with the desktop ears instance
    let socket_path = socket.map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::env::var("XDG_RUNTIME_DIR")
            .map(|d| std::path::PathBuf::from(d).join("ears-ws.sock"))
            .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/ears-ws.sock"))
    });

    // Do NOT kill existing VAD — ws-listen runs alongside the desktop ears instance

    // Build the VAD pipeline components manually (like start_vad_pipeline but without pw-record)
    let language = ears::KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());
    // Live partials decode the growing utterance the way the desktop ghost
    // does; with `live_decoding = "continuous"` the server streams them.
    let continuous =
        (live && config.live_decoding == ears::config::LiveDecoding::Continuous).then(|| {
            ears::continuous::ContinuousSpec {
                server_url: server_url.to_string(),
                api_key: config.api_key.clone(),
                model: model.clone(),
                language: language.clone(),
                context: config.prompt.clone(),
                rollback_words: config.live_rollback_words,
                min_step_ms: config.min_step_ms(),
            }
        });
    let whisper_client = std::sync::Arc::new(
        ears::WhisperClient::new(server_url.to_string())
            .with_language(language)
            .with_api_key(config.api_key.clone())
            .with_model(model)
            .with_silence_filter(config.filter_silence_artifacts)
            .with_prompt(config.prompt.clone()),
    );

    whisper_client
        .health_check()
        .await
        .map_err(|e| anyhow::anyhow!("Whisper server health check failed: {}", e))?;

    // Audio channel — WebSocket server writes here, engine reads
    let (audio_tx, mut audio_rx) = ears::pipeline::audio_channel();

    // Broadcast channel for streaming events — shared by IPC server and WebSocket echo
    let (ipc_tx, ipc_rx) = tokio::sync::broadcast::channel(100);

    // Start WebSocket server (echoes events back to connected clients)
    let ws_handle = ears::ws_input::start_ws_server(host, port, audio_tx, ipc_tx.clone()).await?;

    // Create streaming engine
    let streaming_config = ears::streaming::StreamingConfig::default();
    let vad_config = ears::vad::VadConfig::from_settings(&config.vad);
    let typing_config = ears::progressive_typing::ProgressiveTypingConfig::default();
    let temp_dir = config.state_dir.clone();
    let mut engine = ears::streaming_engine::StreamingEngine::new(
        whisper_client,
        streaming_config,
        vad_config,
        typing_config,
        temp_dir,
    )
    .map_err(|e| anyhow::anyhow!("Failed to create streaming engine: {}", e))?;

    let (event_tx, mut event_rx) =
        tokio::sync::mpsc::unbounded_channel::<ears::streaming_engine::StreamingEvent>();
    engine.set_event_sender(event_tx.clone());
    engine.set_keep_audio_dir(keep_audio);
    if live {
        // Typing is off below and the claim is held, so the ghost machinery
        // never draws into a window: every partial is published as an event
        // instead (`ghost_poll_partials`, the muted + claimed branch).
        engine.set_continuous(continuous);
        engine.set_ghost(true);
        engine.set_claimed(true);
    }
    if say_echo {
        engine.set_say_echo(ears::say_echo::SayEchoConfig {
            enabled: true,
            ..config.say_echo.clone()
        });
    }

    // Disable typing for ws-listen — only emit IPC events, never type into windows
    {
        let (_settings_tx, mut settings_rx) =
            tokio::sync::watch::channel(ears::tui::TypingSettings {
                progressive_typing: false,
                auto_correction: false,
                typing_mode: ears::TypingMode::None,
                auto_enter: false,
                text_filters: config.text_filters.clone(),
                language: config.language.clone(),
                guided_grammar: config.active_grammar(),
                ghost: false,
                claimed: false,
                wake: None,
                command: None,
                colour: None,
            });
        // Apply initial settings
        let s = settings_rx.borrow_and_update().clone();
        engine.set_typing_enabled(
            s.progressive_typing,
            s.auto_correction,
            s.typing_mode,
            s.auto_enter,
        );
        engine.set_text_filters(s.text_filters, s.language);
        engine.set_guided_grammar(s.guided_grammar.clone());

        // Shutdown channel
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        // Audio processing task
        let pipeline_handle = tokio::spawn(ears::pipeline::until_shutdown(
            shutdown_rx,
            async move {
                loop {
                    tokio::select! {
                        audio = audio_rx.recv() => {
                            match audio {
                                Some(samples) => {
                                    if let Err(e) = engine.process_audio(&samples).await {
                                        tracing::warn!("Audio processing error: {}", e);
                                    }
                                }
                                None => {
                                    tracing::debug!("Audio channel closed");
                                    break;
                                }
                            }
                        }
                        Ok(()) = settings_rx.changed() => {
                            let s = settings_rx.borrow_and_update().clone();
                            engine.set_typing_enabled(s.progressive_typing, s.auto_correction, s.typing_mode, s.auto_enter);
                            engine.set_text_filters(s.text_filters, s.language);
                            engine.set_guided_grammar(s.guided_grammar.clone());
                        }
                    }
                }
            },
        ));

        // IPC server on custom socket path (avoids conflict with desktop ears)
        let ipc_server = ears::ipc::start_ipc_server_at(socket_path.clone(), ipc_rx);

        let ws_save_to_clipboard = config.save_to_clipboard;
        tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = ipc_tx.send(event.clone());
                match event {
                    ears::streaming_engine::StreamingEvent::SegmentCompleted {
                        text,
                        duration_ms,
                        ..
                    } => {
                        tracing::info!("Segment: \"{}\" ({}ms)", text, duration_ms);
                        if ws_save_to_clipboard && !text.is_empty() {
                            TextInput::copy_to_clipboard(&text);
                        }
                    }
                    ears::streaming_engine::StreamingEvent::Error(msg) => {
                        tracing::warn!("Streaming error: {}", msg);
                    }
                    _ => {}
                }
            }
        });

        eprintln!("WebSocket VAD listening on ws://{}:{}", host, port);

        // Wait for signal
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())?;
        let mut sigint = signal(SignalKind::interrupt())?;

        tokio::select! {
            _ = sigterm.recv() => {}
            _ = sigint.recv() => {}
        }

        let _ = shutdown_tx.send(true);
        ears::pipeline::join_stopped(pipeline_handle).await;
        ws_handle.abort();
        ipc_server.shutdown().await;
    }

    Ok(())
}

/// Main toggle logic: start recording or stop and transcribe
async fn handle_toggle(config: &Config, ghost: bool) -> Result<()> {
    // Serialize toggle operations across processes to prevent races on the
    // state file and audio file when the keybind is pressed rapidly.
    let lock_path = config.state_dir.join("toggle.lock");
    let mut lock = FileLock::new(&lock_path).context("Failed to create toggle lock")?;
    if !lock.try_lock().context("Failed to check toggle lock")? {
        tracing::info!("Toggle already in progress, ignoring");
        return Ok(());
    }

    let mut state_mgr =
        StateManager::new(&config.state_dir).context("Failed to initialize state manager")?;

    state_mgr.load_state().context("Failed to load state")?;

    let pid_file = config.state_dir.join("recording.pid");
    let process_mgr = ProcessManager::new(&pid_file, Duration::from_secs(120));

    if let Err(e) = process_mgr.cleanup_stale() {
        tracing::debug!("Stale process cleanup: {}", e);
    }

    // If VAD is active, toggle should not interfere
    if state_mgr.current_state() == StateEnum::VadActive {
        tracing::info!("VAD is active, toggle ignored");
        return Ok(());
    }

    state_mgr
        .reconcile_state(|| {
            process_mgr
                .is_recording_alive()
                .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
        })
        .context("Failed to reconcile state")?;

    let is_recording = process_mgr.is_recording_alive().unwrap_or(false);

    if is_recording {
        stop_and_transcribe(config, &mut state_mgr, &process_mgr).await
    } else {
        start_recording(config, &mut state_mgr, &process_mgr, ghost).await
    }
}

/// Start a new recording
async fn start_recording(
    config: &Config,
    state_mgr: &mut StateManager,
    process_mgr: &ProcessManager,
    ghost: bool,
) -> Result<()> {
    let toggle_start = std::time::Instant::now();
    tracing::info!("Starting recording");

    let health_start = std::time::Instant::now();
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    let (server_url, model) = config.resolve_server(language.as_deref());
    let client = WhisperClient::new(server_url.to_string())
        .with_language(language)
        .with_api_key(config.api_key.clone())
        .with_model(model)
        .with_silence_filter(config.filter_silence_artifacts)
        .with_prompt(config.prompt.clone());
    if client.health_check().await.is_err() {
        tracing::error!("Whisper server health check failed");
        AudioFeedback::beep_error().ok();
        Notifications::error("Whisper server not running!").ok();
        anyhow::bail!("Whisper server not available");
    }

    tracing::info!("Health check passed in {:?}", health_start.elapsed());

    let audio_file = config.state_dir.join("recording.wav");
    if audio_file.exists() {
        if let Err(e) = std::fs::remove_file(&audio_file) {
            tracing::warn!("Failed to remove old recording file: {}", e);
        }
    }
    // Settled text of an older recording must never prefix this one.
    let _ = std::fs::remove_file(ghost_continuous_state_file(config));

    state_mgr
        .transition(StateEnum::Recording)
        .context("Failed to transition to Recording state")?;

    let pid = process_mgr
        .spawn_recording(&config.device, &audio_file)
        .context("Failed to start recording")?;

    AudioFeedback::beep_start().ok();

    // Typing disabled in the config: nothing may reach the focused window,
    // ghost text included.
    if ghost && config.typing_mode != ears::desktop::TypingMode::None {
        ears::ghost_style::apply(&config.ghost);
        if let Err(e) = spawn_ghost_preview(config, pid) {
            // Recording still works; the text is just typed at the end.
            tracing::warn!("Ghost preview not started: {}", e);
        }
    }

    tracing::info!(
        "Recording started (PID: {}) total start_recording: {:?}",
        pid,
        toggle_start.elapsed()
    );

    Ok(())
}

/// `ears ghost-style`: show or change the ghost style and write it into the
/// apps that support it.
fn ghost_style_command(
    mut config: Config,
    color: Option<String>,
    underline: bool,
    no_underline: bool,
    frozen: Option<String>,
) -> Result<()> {
    use ears::ghost_style::{apply, normalize_color, PRESETS};
    let parse = |color: String| -> Result<Option<String>> {
        if color.eq_ignore_ascii_case("default") {
            return Ok(None);
        }
        if normalize_color(&color).is_none() {
            let names: Vec<_> = PRESETS.iter().map(|(n, _)| *n).collect();
            anyhow::bail!(
                "not a colour: {color:?} (use #rrggbb, #rgb, default, or one of {})",
                names.join(", ")
            );
        }
        Ok(Some(color))
    };
    let mut changed = false;
    if let Some(color) = color {
        config.ghost.color = parse(color)?;
        changed = true;
    }
    if let Some(color) = frozen {
        config.ghost.frozen_color = parse(color)?;
        changed = true;
    }
    if underline || no_underline {
        config.ghost.underline = underline;
        changed = true;
    }
    if changed {
        config.save().context("Failed to save config")?;
    }
    println!(
        "Ghost text: {}{}, frozen: {}",
        config.ghost.label(),
        if config.ghost.underline {
            ", underlined"
        } else {
            ""
        },
        config.ghost.frozen_label()
    );
    let applied = apply(&config.ghost);
    if applied.is_empty() {
        println!("No supported app found (Alacritty with the preedit patch, Hover).");
    }
    for a in applied {
        let note = match (a.app, a.changed) {
            ("hover", true) => "updated; takes effect when Hover restarts",
            (_, true) => "updated",
            (_, false) => "already set",
        };
        println!("  {:<9} {} ({})", a.app, a.path.display(), note);
    }
    println!("Chromium, Firefox and GTK4 apps (Walker) draw ghost text in their own style.");
    Ok(())
}

/// Stop recording and transcribe
async fn stop_and_transcribe(
    config: &Config,
    state_mgr: &mut StateManager,
    process_mgr: &ProcessManager,
) -> Result<()> {
    let total_start = std::time::Instant::now();
    tracing::info!("Stopping recording and transcribing");

    let pid = match process_mgr.read_pid()? {
        Some(pid) => pid,
        None => {
            AudioFeedback::beep_error().ok();
            Notifications::info("No active recording").ok();
            tracing::warn!("No recording PID found");
            return Ok(());
        }
    };

    if !process_mgr.is_process_alive(pid) {
        AudioFeedback::beep_error().ok();
        Notifications::info("No active recording").ok();
        if let Err(e) = process_mgr.cleanup_stale() {
            tracing::debug!("Stale process cleanup: {}", e);
        }
        tracing::warn!("Recording process not alive (PID: {})", pid);
        return Ok(());
    }

    // Identity of this recording's recorder, taken while it still runs: only
    // a preview of this very recording may hand over settled text.
    let recording = proc_start_time(pid as i32).map(|start| recording_owner(pid as i32, start));

    // A ghost preview belongs to this recording: stop it first so no late
    // partial can land after the final commit. Its last ghost is re-shown
    // below and replaced by the committed text.
    let ghost_session = stop_ghost_preview(config);

    let stop_start = std::time::Instant::now();
    process_mgr
        .stop_recording()
        .context("Failed to stop recording")?;

    // The recorder is stopped: every later return/cancellation must release
    // its audio and reset state, including validation and metadata failures.
    let _state_guard = ears::state::StateResetGuard::new(&config.state_dir);
    let audio_file = config.state_dir.join("recording.wav");
    let _audio_cleanup = owned_file::CleanupFile::new(&audio_file)?;

    tokio::time::sleep(Duration::from_millis(300)).await;
    tracing::info!("Recording stopped in {:?}", stop_start.elapsed());

    if !audio_file.exists() {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is empty or missing").ok();
        tracing::error!("Audio file not found: {}", audio_file.display());
        anyhow::bail!("Audio file not found");
    }

    let metadata = tokio::fs::metadata(&audio_file).await?;

    if metadata.len() <= ears::WAV_HEADER_SIZE {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording too short").ok();
        tracing::error!(
            "Audio file has no audio data ({} bytes, header is {})",
            metadata.len(),
            ears::WAV_HEADER_SIZE
        );
        anyhow::bail!("Recording file contains no audio data");
    }

    tracing::info!("Audio file size: {} bytes", metadata.len());

    let mut header = [0u8; 12];
    let mut file = tokio::fs::File::open(&audio_file).await?;
    use tokio::io::AsyncReadExt;
    file.read_exact(&mut header).await?;

    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        AudioFeedback::beep_error().ok();
        Notifications::error("Recording file is corrupted").ok();
        tracing::error!("Audio file is not a valid WAV file");
        anyhow::bail!("Invalid audio file format");
    }

    tracing::info!("WAV file validation passed");

    state_mgr
        .transition(StateEnum::Transcribing)
        .context("Failed to transition to Transcribing state")?;

    let lang_start = std::time::Instant::now();
    let language = KeyboardLayout::detect_language().or_else(|| config.language.clone());
    tracing::info!("Language detection took {:?}", lang_start.elapsed());
    if let Some(ref lang) = language {
        tracing::info!("Using language: {} (from keyboard layout)", lang);
    }

    let transcribe_start = std::time::Instant::now();
    let (server_url, model) = config.resolve_server(language.as_deref());
    let client = WhisperClient::new(server_url.to_string())
        .with_language(language.clone())
        .with_api_key(config.api_key.clone())
        .with_model(model.clone())
        .with_silence_filter(config.filter_silence_artifacts)
        .with_prompt(config.prompt.clone());
    let grammar = config.active_grammar();
    // Only a recording that had a live preview has settled text to finish;
    // without one a full transcription is just as fast.
    let continuous_final = if ghost_session.is_some()
        && config.live_decoding == ears::config::LiveDecoding::Continuous
        && !config.final_correction
        && grammar.is_none()
    {
        finish_continuous(
            config,
            &audio_file,
            recording.as_deref(),
            server_url.as_str(),
            model,
            language.as_deref(),
        )
        .await?
    } else {
        None
    };
    let _ = std::fs::remove_file(ghost_continuous_state_file(config));
    let result = match continuous_final {
        Some(text) => Ok(text),
        None => {
            client
                .transcribe_with_grammar(&audio_file, grammar.as_deref())
                .await
        }
    };
    match result {
        Ok(text) if !text.is_empty() => {
            tracing::info!(
                "Transcription completed in {:?}: {}",
                transcribe_start.elapsed(),
                text
            );

            // Bash mode bypasses text filters — the grammar already guarantees
            // valid shell syntax and the filters would mangle it.
            let filtered_text = if config.bash_mode {
                ears::bash::clean(&text)
            } else {
                config.text_filters.apply(&text, language.as_deref())
            };
            tracing::debug!("Filtered text: {}", filtered_text);

            let typing_start = std::time::Instant::now();
            use ears::ghost::Delivery;
            let delivery = match ghost_session {
                Some(last) if config.typing_mode != ears::desktop::TypingMode::None => {
                    deliver_via_ghost(&last, &filtered_text)
                }
                _ => Delivery::NotDelivered,
            };
            let typed = match delivery {
                Delivery::Delivered => Ok(()),
                Delivery::NotDelivered => TextInput::type_text(&filtered_text, config.typing_mode),
                // Possibly delivered already; typing again could duplicate it.
                Delivery::Unknown => Err(anyhow::anyhow!(
                    "ghost commit outcome unknown; text not retyped: {}",
                    filtered_text
                )),
            };
            match typed {
                Ok(()) => {
                    if config.auto_enter {
                        if let Err(e) = TextInput::send_enter() {
                            tracing::warn!("Failed to send Enter key: {}", e);
                        }
                    }
                    tracing::info!(
                        "Text typing completed in {:?} ({} chars)",
                        typing_start.elapsed(),
                        filtered_text.len()
                    );
                    AudioFeedback::beep_done().ok();
                }
                Err(e) => {
                    tracing::error!("Failed to type text in {:?}: {}", typing_start.elapsed(), e);
                    AudioFeedback::beep_error().ok();
                    Notifications::error(&format!("Failed to type text: {}", e)).ok();
                }
            }

            // Copy to clipboard if enabled
            if config.save_to_clipboard && !filtered_text.is_empty() {
                TextInput::copy_to_clipboard(&filtered_text);
            }

            post_transcribe::run(&audio_file, &filtered_text);
        }
        Ok(_) => {
            AudioFeedback::beep_error().ok();
            Notifications::info("No speech detected").ok();
            tracing::info!("No speech detected");
        }
        Err(e) => {
            AudioFeedback::beep_error().ok();
            Notifications::error(&format!("Transcription failed: {}", e)).ok();
            tracing::error!("Transcription failed: {}", e);

            // Guard will reset state to Idle on drop
            return Err(e.into());
        }
    }

    // Explicit transition (guard also resets on drop, but this is the clean path)
    state_mgr
        .transition(StateEnum::Idle)
        .context("Failed to transition to Idle state")?;

    tracing::info!("Total stop_and_transcribe: {:?}", total_start.elapsed());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_long_running_modes_rotate_debug_log() {
        let vad = |action| Some(Commands::Vad { action });
        // TUI and ws-listen always run long.
        assert!(owns_debug_log(&None, || unreachable!()));
        // VAD and ghost own it when they start VAD...
        assert!(owns_debug_log(&vad(None), || false));
        assert!(owns_debug_log(&vad(Some(VadAction::Start)), || false));
        assert!(owns_debug_log(&Some(Commands::Ghost), || false));
        // ...not when the toggle stops a running one, or start is a no-op.
        assert!(!owns_debug_log(&vad(None), || true));
        assert!(!owns_debug_log(&vad(Some(VadAction::Start)), || true));
        assert!(!owns_debug_log(&Some(Commands::Ghost), || true));
        // Short-lived subcommands never rotate.
        assert!(!owns_debug_log(&vad(Some(VadAction::Stop)), || false));
        assert!(!owns_debug_log(&vad(Some(VadAction::Status)), || false));
        assert!(!owns_debug_log(
            &Some(Commands::Toggle { ghost: false }),
            || false
        ));
    }

    #[test]
    fn log_filter_keeps_ort_quiet_unless_rust_log_names_it() {
        use tracing::level_filters::LevelFilter;
        let directives = |rust_log| log_filter(rust_log).to_string();
        assert_eq!(directives(None), directives(Some("info")));
        assert!(directives(None).contains("ort=warn"));
        assert!(directives(Some("debug")).contains("ort=warn"));
        assert!(directives(Some("info,ort=info")).contains("ort=info"));
        assert!(!directives(Some("info,ort=info")).contains("ort=warn"));
        assert_eq!(log_filter(None).max_level_hint(), Some(LevelFilter::INFO));
        assert_eq!(
            log_filter(Some("info,ears::health=debug")).max_level_hint(),
            Some(LevelFilter::DEBUG)
        );
        // An unparsable RUST_LOG falls back to the default instead of silence.
        assert_eq!(directives(Some("ears=loud")), directives(None));
    }

    #[test]
    fn default_log_filter_drops_ort_info_and_ears_debug() {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        #[derive(Clone, Default)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let logged = |rust_log| {
            let sink = Sink::default();
            let writer = sink.clone();
            let subscriber = tracing_subscriber::registry()
                .with(log_filter(rust_log))
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_ansi(false)
                        .with_writer(move || writer.clone()),
                );
            tracing::subscriber::with_default(subscriber, || {
                tracing::info!(target: "ort::logging", "ort-info");
                tracing::warn!(target: "ort::logging", "ort-warn");
                tracing::debug!(target: "ears", "ears-debug");
                tracing::info!(target: "ears::vad", "ears-info");
            });
            let bytes = sink.0.lock().unwrap().clone();
            String::from_utf8(bytes).unwrap()
        };
        let has = |log: &str, names: [&str; 4]| names.map(|name| log.contains(name));
        let names = ["ort-info", "ort-warn", "ears-debug", "ears-info"];
        assert_eq!(has(&logged(None), names), [false, true, false, true]);
        assert_eq!(
            has(&logged(Some("debug")), names),
            [false, true, true, true]
        );
        assert_eq!(
            has(&logged(Some("info,ort=info")), names),
            [true, true, false, true]
        );
    }

    #[test]
    fn test_set_server_validation() {
        // Test URL validation without calling Config::load/save
        let test_url = "http://localhost:9999";
        let parsed = Url::parse(test_url).unwrap();
        assert_eq!(parsed.scheme(), "http");
        assert_eq!(parsed.host_str(), Some("localhost"));
        assert_eq!(parsed.port(), Some(9999));
        assert_eq!(parsed.as_str(), "http://localhost:9999/");
    }

    #[test]
    fn test_invalid_url() {
        assert!(Url::parse("not-a-url").is_err());
    }

    #[test]
    fn test_mask_secret() {
        // Long secret keeps a prefix/suffix, hides the middle and never leaks it.
        let masked = mask_secret("gsk_abcdefghijklmnopqrstuvwxyz0123");
        assert_eq!(masked, "set (gsk_…123)");
        assert!(!masked.contains("abcdef"));

        // Short secrets reveal nothing.
        assert_eq!(mask_secret("short"), "set (hidden)");
        assert_eq!(mask_secret("12345678"), "set (hidden)");
    }

    #[test]
    fn test_invalid_scheme() {
        let url = Url::parse("ftp://localhost:8080").unwrap();
        assert!(!matches!(url.scheme(), "http" | "https"));
    }
}
