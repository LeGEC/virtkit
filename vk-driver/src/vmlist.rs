//! `vk list`: one row per VM `vk` knows — every running VM, every named VM that is stopped,
//! and each compose service of either — grouped by project, a primary before its services.
//!
//! [`rows`] is pure: it turns what the caller read (the registry's live entries with what was
//! measured of them, and the named VMs that are not running) into rows, so a test states the
//! facts outright. [`render`] lays the rows out as the table.

use std::path::{Path, PathBuf};

use serde::Serialize;
use vk_core::fleetctl::UnitStatus;

use crate::vms::{Freshness, Published, VmEntry};

/// What a VM is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// a `vk dev` environment
    Dev,
    /// a `vk run`
    Run,
    /// a compose service of either
    Service,
}

impl Kind {
    fn label(self) -> &'static str {
        match self {
            Kind::Dev => "dev",
            Kind::Run => "run",
            Kind::Service => "service",
        }
    }
}

/// The status of a VM that is not running.
pub const STOPPED: &str = "stopped";

/// One `vk list` row; `vk list --json` is an array of these.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    /// what every VM command takes: the VM's name, `NAME/SERVICE` for a compose service, or
    /// the pid of a running VM that has no name
    pub name: String,
    pub kind: Kind,
    /// `running` or `stopped`; a service may report another state of its own
    pub status: String,
    /// the VM a compose service belongs to
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// the managing `vk run`'s pid, while it runs
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    /// what the VM's process tree holds on the host now, services included
    pub mem_used_bytes: Option<u64>,
    /// the memory size it booted with
    pub mem: Option<String>,
    /// what it boots: the image, the compose primary or the Dockerfile
    pub image: Option<String>,
    /// the project it is about: a dev environment's workspace, a run's project directory
    pub project: Option<PathBuf>,
    /// a dev environment's environment name
    pub environment: Option<String>,
    pub state_dir: PathBuf,
    pub exec_addr: Option<String>,
    pub ssh_addr: Option<String>,
    /// its address on the run's LAN
    pub ip: Option<String>,
    /// the ports published to the host, `listen->to`
    pub published: Vec<String>,
    /// with `--stale`: whether a fresh `vk run` would rebuild its image (`null`: unknown)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<Option<bool>>,
}

/// A running VM, with what was read of it.
pub struct Live<'a> {
    pub entry: &'a VmEntry,
    pub name: Option<String>,
    pub kind: Kind,
    pub environment: Option<String>,
    /// its services' states; `None` when the VM could not be asked
    pub units: Option<Vec<UnitStatus>>,
    pub published: Vec<Published>,
    pub mem_used: Option<u64>,
    /// with `--stale`, its freshness
    pub fresh: Option<Freshness>,
}

/// A named VM that is not running.
pub struct Known {
    pub name: String,
    pub dir: PathBuf,
    pub kind: Kind,
    /// the registry entry it last ran with, if it left one
    pub last: Option<VmEntry>,
    pub project: Option<PathBuf>,
    pub environment: Option<String>,
}

/// Every row, grouped by project (groups by path, no project last), dev environments before
/// runs within a group, each primary followed by its services by name.
pub fn rows(live: &[Live], known: &[Known], now: u64) -> Vec<Row> {
    let mut out: Vec<Row> = Vec::new();
    for l in live {
        let e = l.entry;
        let name = l.name.clone().unwrap_or_else(|| e.pid.to_string());
        let (own, by_service): (Vec<&Published>, Vec<&Published>) =
            l.published.iter().partition(|(p, _)| p.service.is_none());
        out.push(Row {
            name: name.clone(),
            kind: l.kind,
            status: "running".into(),
            parent: None,
            pid: Some(e.pid),
            uptime_secs: Some(now.saturating_sub(e.created_secs)),
            mem_used_bytes: l.mem_used,
            mem: e.mem.clone(),
            image: Some(e.label.clone()),
            project: e.project_dir.clone(),
            environment: l.environment.clone(),
            state_dir: e.state_dir.clone(),
            exec_addr: Some(e.exec_addr.clone()),
            ssh_addr: e.ssh_addr.clone(),
            ip: e.guest_ip.map(|ip| ip.to_string()),
            published: own.into_iter().map(published_text).collect(),
            stale: l.fresh.map(Freshness::json),
        });
        for s in &e.services {
            let unit = l.units.as_ref().and_then(|u| u.iter().find(|u| u.name == s.name));
            out.push(Row {
                name: format!("{name}/{}", s.name),
                kind: Kind::Service,
                status: unit.map_or_else(|| "-".into(), |u| u.state.clone()),
                parent: Some(name.clone()),
                pid: None,
                uptime_secs: None,
                mem_used_bytes: None,
                mem: None,
                image: None,
                project: e.project_dir.clone(),
                environment: l.environment.clone(),
                state_dir: service_dir(&s.exec_addr).unwrap_or_else(|| e.state_dir.clone()),
                exec_addr: Some(s.exec_addr.clone()),
                ssh_addr: None,
                ip: unit.map(|u| crate::vms::bare_ip(&u.ip).to_string()),
                published: by_service
                    .iter()
                    .filter(|(p, _)| p.service.as_deref() == Some(s.name.as_str()))
                    .map(|p| published_text(p))
                    .collect(),
                stale: None,
            });
        }
    }
    for k in known {
        let last = k.last.as_ref();
        out.push(Row {
            name: k.name.clone(),
            kind: k.kind,
            status: STOPPED.into(),
            parent: None,
            pid: None,
            uptime_secs: None,
            mem_used_bytes: None,
            mem: last.and_then(|e| e.mem.clone()),
            image: last.map(|e| e.label.clone()),
            project: k
                .project
                .clone()
                .or_else(|| last.and_then(|e| e.project_dir.clone())),
            environment: k.environment.clone(),
            state_dir: k.dir.clone(),
            exec_addr: None,
            ssh_addr: None,
            ip: None,
            published: Vec::new(),
            stale: None,
        });
        let project = out.last().and_then(|r| r.project.clone());
        for s in last.map(|e| e.services.as_slice()).unwrap_or_default() {
            out.push(Row {
                name: format!("{}/{}", k.name, s.name),
                kind: Kind::Service,
                status: STOPPED.into(),
                parent: Some(k.name.clone()),
                pid: None,
                uptime_secs: None,
                mem_used_bytes: None,
                mem: None,
                image: None,
                project: project.clone(),
                environment: k.environment.clone(),
                state_dir: k.dir.join(format!("svc-{}", s.name)),
                exec_addr: None,
                ssh_addr: None,
                ip: None,
                published: Vec::new(),
                stale: None,
            });
        }
    }
    let kind_of: std::collections::HashMap<String, Kind> = out
        .iter()
        .filter(|r| r.parent.is_none())
        .map(|r| (r.name.clone(), r.kind))
        .collect();
    out.sort_by(|a, b| sort_key(a, &kind_of).cmp(&sort_key(b, &kind_of)));
    out
}

/// Where a row sorts: its project (none last), its primary's kind and name, then the
/// primary before its services, services by name.
fn sort_key(
    r: &Row,
    kind_of: &std::collections::HashMap<String, Kind>,
) -> (bool, Option<PathBuf>, Kind, String, bool, String) {
    let primary = r.parent.clone().unwrap_or_else(|| r.name.clone());
    let kind = kind_of.get(&primary).copied().unwrap_or(r.kind);
    (
        r.project.is_none(),
        r.project.clone(),
        kind,
        primary,
        r.parent.is_some(),
        r.name.clone(),
    )
}

/// The service directory an exec address `vsock-auto://<svc-dir>/vsock.sock:4444` names.
fn service_dir(exec_addr: &str) -> Option<PathBuf> {
    let path = exec_addr.strip_prefix("vsock-auto://")?;
    let (sock, _port) = path.rsplit_once(':')?;
    Path::new(sock).parent().map(Path::to_path_buf)
}

fn published_text((e, liveness): &Published) -> String {
    let mut cell = crate::vms::published_addr(e);
    if liveness.unconfirmed() {
        cell.push_str(" (unconfirmed)");
    }
    cell
}

/// Which rows to keep.
#[derive(Debug, Default, Clone)]
pub struct Filter {
    pub running: bool,
    pub stopped: bool,
    pub kind: Option<Kind>,
    /// only this row (a `NAME/SERVICE`)
    pub name: Option<String>,
}

impl Filter {
    pub fn keeps(&self, r: &Row) -> bool {
        (!self.running || r.status != STOPPED)
            && (!self.stopped || r.status == STOPPED)
            && self.kind.is_none_or(|k| k == r.kind)
            && self.name.as_ref().is_none_or(|n| *n == r.name)
    }
}

/// The table: NAME, KIND, STATUS, PID, UPTIME, MEM, IMAGE, PROJECT, then EXEC ADDRESS with
/// `wide`, PUBLISHED, and STALE with `stale`. `home` folds into `~` in PROJECT.
pub fn render(rows: &[Row], home: Option<&Path>, wide: bool, stale: bool) -> String {
    let dash = || "-".to_string();
    let mut headers = vec![
        "NAME", "KIND", "STATUS", "PID", "UPTIME", "MEM", "IMAGE", "PROJECT",
    ];
    if wide {
        headers.push("EXEC ADDRESS");
    }
    headers.push("PUBLISHED");
    if stale {
        headers.push("STALE");
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| {
            let mut row = vec![
                r.name.clone(),
                r.kind.label().to_string(),
                r.status.clone(),
                r.pid.map_or_else(dash, |p| p.to_string()),
                r.uptime_secs.map_or_else(dash, crate::vms::fmt_uptime),
                match (r.parent.is_some(), r.status.as_str()) {
                    (true, _) | (_, STOPPED) => dash(),
                    _ => crate::vms::mem_cell(r.mem_used_bytes, r.mem.as_deref()),
                },
                r.image.clone().unwrap_or_else(dash),
                crate::vms::project_cell(r.project.as_deref(), home, wide),
            ];
            if wide {
                row.push(r.exec_addr.clone().unwrap_or_else(dash));
            }
            row.push(match r.published.is_empty() {
                true => dash(),
                false => r.published.join(", "),
            });
            if stale {
                row.push(match r.stale {
                    Some(Some(true)) => "stale".into(),
                    Some(Some(false)) => "fresh".into(),
                    _ => dash(),
                });
            }
            row
        })
        .collect();
    crate::dev::list::table(&headers, &cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &str, project: &str, pid: u32, services: &[&str]) -> VmEntry {
        let mut e = crate::vms::tests::entry(PathBuf::from(dir), Some(PathBuf::from(project)));
        e.pid = pid;
        e.label = format!("img-{pid}");
        e.created_secs = 100;
        e.services = services
            .iter()
            .map(|s| crate::vms::ServiceEntry {
                name: s.to_string(),
                exec_addr: format!("vsock-auto://{dir}/svc-{s}/vsock.sock:4444"),
                stale_recipe: None,
            })
            .collect();
        e
    }

    fn unit(name: &str, state: &str) -> UnitStatus {
        UnitStatus {
            name: name.into(),
            state: state.into(),
            ip: "10.0.0.5/24".into(),
        }
    }

    fn live<'a>(e: &'a VmEntry, name: &str, kind: Kind, units: Vec<UnitStatus>) -> Live<'a> {
        Live {
            entry: e,
            name: Some(name.into()),
            kind,
            environment: None,
            units: Some(units),
            published: Vec::new(),
            mem_used: Some(1 << 30),
            fresh: None,
        }
    }

    fn names(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn rows_group_by_project_dev_first_primary_then_its_services_by_name() {
        let app = entry("/s/app", "/w/app", 10, &["web", "db"]);
        let tool = entry("/s/tool", "/w/app", 11, &[]);
        let other = entry("/s/other", "/w/other", 12, &[]);
        let stopped = Known {
            name: "zeta".into(),
            dir: "/s/zeta".into(),
            kind: Kind::Run,
            last: Some(entry("/s/zeta", "/w/app", 13, &["cache"])),
            project: None,
            environment: None,
        };
        let rows = rows(
            &[
                live(&tool, "tool", Kind::Run, vec![]),
                live(&other, "other", Kind::Run, vec![]),
                live(&app, "app", Kind::Dev, vec![unit("db", "running")]),
            ],
            &[stopped],
            160,
        );
        assert_eq!(
            names(&rows),
            ["app", "app/db", "app/web", "tool", "zeta", "zeta/cache", "other"]
        );
        let by = |n: &str| rows.iter().find(|r| r.name == n).unwrap();
        assert_eq!(by("app").uptime_secs, Some(60));
        assert_eq!(by("app/db").status, "running");
        assert_eq!(by("app/db").ip.as_deref(), Some("10.0.0.5"));
        assert_eq!(by("app/db").state_dir, PathBuf::from("/s/app/svc-db"));
        // Declared but not reported: unknown, not stopped.
        assert_eq!(by("app/web").status, "-");
        assert_eq!(by("zeta").status, STOPPED);
        assert_eq!(by("zeta").image.as_deref(), Some("img-13"));
        assert_eq!(by("zeta/cache").status, STOPPED);
        assert_eq!(by("zeta/cache").parent.as_deref(), Some("zeta"));
    }

    #[test]
    fn an_unnamed_running_vm_is_named_by_its_pid() {
        let e = entry("/s/x", "/w/x", 4242, &["db"]);
        let mut l = live(&e, "ignored", Kind::Run, vec![]);
        l.name = None;
        let rows = rows(&[l], &[], 0);
        assert_eq!(names(&rows), ["4242", "4242/db"]);
    }

    #[test]
    fn filters_keep_by_status_kind_and_name() {
        let app = entry("/s/app", "/w/app", 10, &["db", "web"]);
        let rows = rows(
            &[live(&app, "app", Kind::Run, vec![unit("db", "stopped"), unit("web", "running")])],
            &[],
            0,
        );
        let keep = |f: Filter| {
            rows.iter()
                .filter(|r| f.keeps(r))
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            keep(Filter {
                running: true,
                ..Filter::default()
            }),
            ["app", "app/web"]
        );
        assert_eq!(
            keep(Filter {
                stopped: true,
                ..Filter::default()
            }),
            ["app/db"]
        );
        assert_eq!(
            keep(Filter {
                kind: Some(Kind::Service),
                ..Filter::default()
            }),
            ["app/db", "app/web"]
        );
        assert_eq!(
            keep(Filter {
                name: Some("app/web".into()),
                ..Filter::default()
            }),
            ["app/web"]
        );
    }

    #[test]
    fn the_table_starts_with_the_name_and_dashes_what_a_row_lacks() {
        let app = entry("/s/app", "/w/app", 10, &["db"]);
        let rows = rows(
            &[live(&app, "app", Kind::Dev, vec![unit("db", "running")])],
            &[],
            160,
        );
        let out = render(&rows, None, false, false);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines[0].starts_with("NAME    KIND"), "{out}");
        assert!(lines[1].starts_with("app     dev"), "{out}");
        assert!(lines[2].starts_with("app/db  service  running  -"), "{out}");
        assert!(render(&rows, None, true, true).lines().next().unwrap().contains("EXEC ADDRESS"));
    }
}
