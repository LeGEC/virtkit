//! `vk start | stop | restart | rm | reset | rename`: the lifecycle of a VM `vk list` names,
//! whatever made it. A dev environment goes through `vk dev` (its config is in its
//! workspace), a run through the recipe it left (`run.json`), a compose service through its
//! primary's service manager.
//!
//! [`plan`] is pure: it says what a verb does to a target in a given state, so a test pins
//! every kind and state; [`run`] carries the plan out.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};

use crate::vmlist::Kind;
use crate::vms::{self, Ref, VmEntry};

/// A VM a lifecycle verb acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// the VM's name (the primary's, for a service)
    pub name: String,
    pub dir: PathBuf,
    /// `Dev` or `Run`: what the VM — the primary, for a service — is
    pub kind: Kind,
    pub service: Option<String>,
}

impl Target {
    fn shown(&self) -> String {
        match &self.service {
            Some(s) => format!("{}/{s}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Resolve a REF to a named VM: a name or `NAME/SERVICE`, or a pid or directory of a running
/// VM that has a name.
pub fn resolve(arg: &OsStr) -> Result<Target> {
    let idx = crate::names::Index::open()?;
    let (name, dir, service) = match Ref::parse(arg)? {
        Ref::Name { vm, service } => match crate::names::resolve(&vm)? {
            Some(dir) => (idx.name_of(&dir).unwrap_or(vm), dir, service),
            None => bail!("no VM is named {vm} (`vk list` names them)"),
        },
        _ => {
            let entry = vms::resolve_one(Some(Path::new(arg)))?;
            let Some(name) = idx.name_of(&entry.state_dir) else {
                bail!(
                    "the VM {} has no name: it is not known once it stops (`vk stop {}`)",
                    entry.pid,
                    entry.pid
                );
            };
            (name, entry.state_dir, None)
        }
    };
    let dev_base = crate::dev::plan::dev_state_base().map(|b| vms::canonical(&b))?;
    let kind = match vms::canonical(&dir).parent() == Some(dev_base.as_path()) {
        true => Kind::Dev,
        false => Kind::Run,
    };
    Ok(Target {
        name,
        dir,
        kind,
        service,
    })
}

/// The running VM on `dir`, if any.
fn running_on(dir: &Path) -> Option<VmEntry> {
    let dir = vms::canonical(dir);
    vms::running().into_iter().find(|e| e.state_dir == dir)
}

/// One step of a verb.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `vk dev --workspace W --environment E up`
    DevUp,
    /// `vk dev … service up|down S`
    DevService { up: bool },
    /// replay `run.json`
    Replay,
    /// a `Start`/`Stop` to the run's service manager
    Control { up: bool },
    /// stop the dev environment, publishers and all (`vk dev stop`)
    DevStop,
    /// SIGTERM the run and wait
    RunStop,
    /// remove the dev environment's state (`vk dev gc`)
    DevRemove,
    /// remove a managed run's state dir
    RemoveDir,
    /// forget a run whose state dir is the user's: its recipe and last entry go, the dir stays
    Forget,
    /// remove a service's `svc-<name>/`
    RemoveService,
    /// say so and do nothing
    Nothing(String),
}

/// The verbs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
    Rm { force: bool },
}

/// What `verb` does to `t`, given whether its primary and (for a service) the service run,
/// and whether its state dir is one `vk` manages.
pub fn plan(
    verb: Verb,
    t: &Target,
    primary_up: bool,
    service_up: bool,
    managed: bool,
) -> Result<Vec<Step>> {
    use Step::*;
    let dev = t.kind == Kind::Dev;
    Ok(match (verb, &t.service) {
        (Verb::Start, None) if primary_up => vec![Nothing(format!("{} is running", t.shown()))],
        (Verb::Start, None) => vec![if dev { DevUp } else { Replay }],
        (Verb::Start, Some(_)) => {
            let mut steps = Vec::new();
            if !primary_up {
                steps.push(if dev { DevUp } else { Replay });
            }
            steps.push(if dev {
                DevService { up: true }
            } else {
                Control { up: true }
            });
            steps
        }
        (Verb::Stop, None) if !primary_up && !dev => {
            vec![Nothing(format!("{} is not running", t.shown()))]
        }
        // `vk dev stop` also takes down what an environment publishes when it is down.
        (Verb::Stop, None) => vec![if dev { DevStop } else { RunStop }],
        (Verb::Stop, Some(_)) if !primary_up || !service_up => {
            vec![Nothing(format!("{} is not running", t.shown()))]
        }
        (Verb::Stop, Some(_)) => vec![if dev {
            DevService { up: false }
        } else {
            Control { up: false }
        }],
        (Verb::Rm { force }, service) => {
            let up = match service {
                None => primary_up,
                Some(_) => primary_up && service_up,
            };
            let mut steps = Vec::new();
            if up {
                if !force {
                    bail!(
                        "{} is running — `vk stop {}` first, or `vk rm -f`",
                        t.shown(),
                        t.shown()
                    );
                }
                steps.extend(plan(Verb::Stop, t, primary_up, service_up, managed)?);
            }
            steps.push(match (service, dev, managed) {
                (Some(_), _, _) => RemoveService,
                (None, true, _) => DevRemove,
                (None, false, true) => RemoveDir,
                (None, false, false) => Forget,
            });
            steps
        }
    })
}

/// Whether `dir` is a state dir `vk` made (under the dev or run base), and so may delete.
fn managed(dir: &Path) -> bool {
    let parent = vms::canonical(dir).parent().map(Path::to_path_buf);
    let base = |b: Result<PathBuf>| b.ok().map(|b| vms::canonical(&b));
    parent.is_some()
        && (parent == base(crate::dev::plan::dev_state_base())
            || parent == base(crate::names::run_base()))
}

/// A dev environment's workspace and environment, as its `dev.json` records them.
fn dev_identity(dir: &Path) -> Result<(String, String)> {
    let path = dir.join("dev.json");
    let identity: crate::dev::Identity = serde_json::from_slice(
        &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("reading {}", path.display()))?;
    let text = |key: &str| {
        identity
            .manifest
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };
    let workspace = text("workspace").context("its dev.json records no workspace")?;
    if !Path::new(&workspace).is_dir() {
        bail!("its workspace {workspace} is gone");
    }
    Ok((
        workspace,
        text("environment").unwrap_or_else(|| "dev".into()),
    ))
}

/// Run this `vk` with `args`, in `cwd`, its output ours; fail unless it succeeds.
fn vk(args: &[String], cwd: Option<&Path>, from_recipe: bool) -> Result<()> {
    let exe = std::env::current_exe().context("resolving the running vk")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    if from_recipe {
        cmd.env(crate::FROM_RECIPE_ENV, "1");
    }
    let status = cmd.status().context("running vk")?;
    if !status.success() {
        bail!("`vk {}` {status}", args.join(" "));
    }
    Ok(())
}

/// Carry out `steps` on `t`.
fn execute(t: &Target, steps: &[Step], timeout: u64, yes: bool) -> Result<String> {
    let mut out = String::new();
    for step in steps {
        match step {
            Step::Nothing(what) => out.push_str(&format!("{what}\n")),
            Step::DevUp => {
                let (ws, env) =
                    dev_identity(&t.dir).with_context(|| format!("cannot start {}", t.name))?;
                let args = ["dev", "--workspace", &ws, "--environment", &env, "up"];
                vk(&args.map(String::from), None, false)?;
            }
            Step::DevService { up } => {
                let (ws, env) = dev_identity(&t.dir)?;
                let service = t.service.clone().unwrap_or_default();
                let op = if *up { "up" } else { "down" };
                let args = [
                    "dev",
                    "--workspace",
                    &ws,
                    "--environment",
                    &env,
                    "service",
                    op,
                    &service,
                ];
                vk(&args.map(String::from), None, false)?;
            }
            Step::Replay => {
                let Some(recipe) = crate::names::Recipe::read(&t.dir)? else {
                    bail!(
                        "{} recorded no way to run it again (no {}): run it with `vk run` once more",
                        t.name,
                        crate::names::RECIPE
                    );
                };
                vk(
                    &recipe.replay_args(&t.dir, &t.name),
                    Some(&recipe.cwd),
                    true,
                )?;
            }
            Step::Control { up } => {
                let entry =
                    running_on(&t.dir).with_context(|| format!("{} is not running", t.name))?;
                let ctl = vms::control_socket(&entry)
                    .with_context(|| format!("{} runs no compose services", t.name))?;
                let unit = t.service.clone().unwrap_or_default();
                let req = match up {
                    true => vk_core::fleetctl::Request::Start { unit },
                    false => vk_core::fleetctl::Request::Stop { unit },
                };
                let timeout = (!up).then(|| std::time::Duration::from_secs(timeout));
                let reply = vms::control(&ctl, &req, timeout, |line| eprintln!("{line}"))?;
                if !reply.ok {
                    bail!("{}: {}", t.shown(), reply.message);
                }
                out.push_str(&format!(
                    "{} {}\n",
                    if *up { "started" } else { "stopped" },
                    t.shown()
                ));
            }
            Step::DevStop => {
                let stopped = crate::dev::stop(&t.dir, timeout)?;
                out.push_str(&stopped.report);
                if !stopped.all_down {
                    bail!("{out}{} did not stop", t.name);
                }
            }
            Step::RunStop => {
                let Some(entry) = running_on(&t.dir) else {
                    out.push_str(&format!("{} is not running\n", t.name));
                    continue;
                };
                let (line, down) = vms::stop_entry(&entry, timeout)
                    .with_context(|| format!("{} is held by another run", t.name))?;
                out.push_str(&line);
                if !down {
                    bail!("{out}");
                }
            }
            Step::DevRemove => {
                let dir_name = t.dir.file_name().unwrap_or_default().to_string_lossy();
                let code = crate::dev::cli::dev_gc(yes, false, &[dir_name.into_owned()], false);
                if code != ExitCode::SUCCESS {
                    bail!("{} was not removed", t.name);
                }
            }
            Step::RemoveDir => {
                let idx = crate::names::Index::open()?;
                if !t.dir.exists() {
                    // A name left on a directory something else removed: only the name goes.
                    idx.remove(&t.name);
                    out.push_str(&format!("forgot {} (its state dir was gone)\n", t.name));
                    continue;
                }
                std::fs::remove_dir_all(&t.dir)
                    .with_context(|| format!("removing {}", t.dir.display()))?;
                idx.release(&t.dir);
                out.push_str(&format!("removed {}\n", t.name));
            }
            Step::Forget => {
                for f in [crate::names::RECIPE, vms::LAST] {
                    let _ = std::fs::remove_file(t.dir.join(f));
                }
                crate::names::Index::open()?.release(&t.dir);
                out.push_str(&format!(
                    "removed {} (its state dir {} is yours and stays)\n",
                    t.name,
                    t.dir.display()
                ));
            }
            Step::RemoveService => {
                let service = t.service.clone().unwrap_or_default();
                let dir = t.dir.join(format!("svc-{service}"));
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => out.push_str(&format!("removed {}\n", t.shown())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        out.push_str(&format!("{} holds no state\n", t.shown()))
                    }
                    Err(e) => {
                        return Err(e).with_context(|| format!("removing {}", dir.display()));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Whether the service `name` of the running `entry` is up.
fn service_up(entry: Option<&VmEntry>, name: Option<&str>) -> bool {
    let (Some(entry), Some(name)) = (entry, name) else {
        return false;
    };
    let Some(ctl) = vms::control_socket(entry) else {
        return false;
    };
    let req = vk_core::fleetctl::Request::Status { unit: name.into() };
    vms::control(&ctl, &req, Some(std::time::Duration::from_secs(5)), |_| {}).is_ok_and(|r| {
        r.ok && r
            .units
            .iter()
            .any(|u| u.name == name && u.state != "stopped")
    })
}

/// `vk start|stop|rm` on each REF in turn; the exit status says whether every one did.
pub fn run(verb: Verb, refs: &[std::ffi::OsString], timeout: u64, yes: bool) -> ExitCode {
    let mut ok = true;
    for r in refs {
        let result = resolve(r).and_then(|t| {
            let entry = running_on(&t.dir);
            let up = service_up(entry.as_ref(), t.service.as_deref());
            let steps = plan(verb, &t, entry.is_some(), up, managed(&t.dir))?;
            execute(&t, &steps, timeout, yes)
        });
        match result {
            Ok(report) => print!("{report}"),
            Err(e) => {
                eprintln!("vk: {e:#}");
                ok = false;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `vk restart REF`: stop, then start.
pub fn restart(r: &OsStr, timeout: u64) -> ExitCode {
    match run(Verb::Stop, &[r.to_os_string()], timeout, false) {
        ExitCode::SUCCESS => run(Verb::Start, &[r.to_os_string()], timeout, false),
        code => code,
    }
}

/// `vk rename OLD NEW`.
pub fn rename(old: &str, new: &str) -> Result<String> {
    let idx = crate::names::Index::open()?;
    if idx.lookup(old).is_none() {
        // A dev environment still known by its state dir's own name gets its first name here.
        let Some(dir) = crate::names::resolve(old)? else {
            bail!("no VM is named {old}");
        };
        idx.claim_exact(&dir, new)?;
    } else {
        idx.rename(old, new)?;
    }
    Ok(format!("renamed {old} to {new}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(kind: Kind, service: Option<&str>) -> Target {
        Target {
            name: "app".into(),
            dir: "/s/app".into(),
            kind,
            service: service.map(Into::into),
        }
    }

    fn steps(
        verb: Verb,
        kind: Kind,
        service: Option<&str>,
        up: bool,
        svc_up: bool,
        managed: bool,
    ) -> Vec<Step> {
        plan(verb, &target(kind, service), up, svc_up, managed).unwrap()
    }

    #[test]
    fn start_boots_by_kind_and_a_service_after_its_primary() {
        use Step::*;
        assert_eq!(
            steps(Verb::Start, Kind::Dev, None, false, false, true),
            [DevUp]
        );
        assert_eq!(
            steps(Verb::Start, Kind::Run, None, false, false, true),
            [Replay]
        );
        assert!(matches!(
            steps(Verb::Start, Kind::Run, None, true, false, true)[..],
            [Nothing(_)]
        ));
        assert_eq!(
            steps(Verb::Start, Kind::Dev, Some("db"), false, false, true),
            [DevUp, DevService { up: true }]
        );
        assert_eq!(
            steps(Verb::Start, Kind::Run, Some("db"), true, false, true),
            [Control { up: true }]
        );
    }

    #[test]
    fn stop_goes_through_dev_for_an_environment_and_skips_what_is_down() {
        use Step::*;
        assert_eq!(
            steps(Verb::Stop, Kind::Dev, None, false, false, true),
            [DevStop]
        );
        assert_eq!(
            steps(Verb::Stop, Kind::Run, None, true, false, true),
            [RunStop]
        );
        assert!(matches!(
            steps(Verb::Stop, Kind::Run, None, false, false, true)[..],
            [Nothing(_)]
        ));
        assert_eq!(
            steps(Verb::Stop, Kind::Dev, Some("db"), true, true, true),
            [DevService { up: false }]
        );
        assert!(matches!(
            steps(Verb::Stop, Kind::Run, Some("db"), true, false, true)[..],
            [Nothing(_)]
        ));
    }

    #[test]
    fn rm_refuses_what_runs_unless_forced_and_never_deletes_a_users_dir() {
        use Step::*;
        assert!(
            plan(
                Verb::Rm { force: false },
                &target(Kind::Run, None),
                true,
                false,
                true
            )
            .is_err()
        );
        assert_eq!(
            steps(Verb::Rm { force: true }, Kind::Run, None, true, false, true),
            [RunStop, RemoveDir]
        );
        assert_eq!(
            steps(
                Verb::Rm { force: false },
                Kind::Run,
                None,
                false,
                false,
                false
            ),
            [Forget]
        );
        assert_eq!(
            steps(
                Verb::Rm { force: false },
                Kind::Dev,
                None,
                false,
                false,
                true
            ),
            [DevRemove]
        );
        assert_eq!(
            steps(
                Verb::Rm { force: false },
                Kind::Run,
                Some("db"),
                true,
                false,
                true
            ),
            [RemoveService]
        );
        assert!(
            plan(
                Verb::Rm { force: false },
                &target(Kind::Run, Some("db")),
                true,
                true,
                true
            )
            .is_err()
        );
    }
}
