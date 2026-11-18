# VM names, one selector, one list, symmetric lifecycle verbs

## Goal

Give every VM `vk` knows a short, stable **name**, accept the same **REF** in every command
that acts on a VM, list every known VM — running or stopped, primary or compose service — in
one `vk list`, and offer `vk start | stop | restart | rm REF` at the top level so a name shown
by `vk list` works with all of them.

## Context

- A run is tracked only while it runs: the VM registry (`$XDG_DATA_HOME/virtkit/vms/`) holds
  one entry per live `--state-dir` run and drops it when the run exits.
- A dev environment keeps a durable state dir (`$XDG_STATE_HOME/virtkit/dev/<ws>[-<env>]-<hash>`)
  with a `dev.json` identity; it is registered like any run while it runs.
- Compose services run in `svc-<name>/` inside their primary's state dir and are reached through
  the run's control socket.
- Commands selected VMs by PID, by project directory (prefix match), by exact state dir, or (dev
  only) by state-dir basename, each command accepting a different subset.
- `vk list` output is not a contract. vk-hub local is released in lockstep with `vk`. The fleet
  `Workload` wire type takes optional field additions only.

## Constraints

- No new dependency.
- Follow the existing `*_in(base)` pattern so unit tests never touch the real XDG dirs.
- Never delete a directory `vk` did not create (a user-chosen `--state-dir`).
- Foreground runs without `--state-dir` stay unregistered and unnamed.

## Design

### Names (`vk-driver/src/names.rs`)

- Grammar: `[a-z0-9][a-z0-9._-]{0,39}`, not all digits. No `/`, `:`, `@` (reserved: `/` for
  services, `:`/`@` for later use).
- Index: `$XDG_STATE_HOME/virtkit/names/<name>`, a symlink to the absolute state dir. Claiming
  a name is `symlink(2)`, atomic, `EEXIST` meaning taken. The index is the single source of
  truth: dir → name is a scan of the index (tens of entries). A `names/.lock` flock serializes
  claims so one directory never gets two names.
- A name is chosen once and kept. On collision the base gets `-2`, `-3`, …
- `vk rename OLD NEW` moves the link. `rm` releases it. A link whose target is gone is skipped
  by readers and removed by `vk rm NAME`.

Default names:

| VM | Default |
|---|---|
| dev environment `dev` | workspace basename (`wab-wt1`) |
| dev environment `E` | `<basename>-E` |
| `vk run --name X` | `X` |
| any other registered `vk run` (`--state-dir`, or `--detach`) | from its label: image basename without tag, compose primary, or Dockerfile target |
| compose service | not named: `<vm>/<service>` |

Dev environments are named **lazily**: any command that resolves or lists names first gives a
name to every dev state dir that has a `dev.json` and none yet, oldest boot first, so existing
hosts migrate without a boot. Ephemeral task dirs (no `dev.json`) get no name. A dev dir's
basename (the former name) keeps resolving as an alias.

### REF (S1)

Read in this order:

1. contains `://` → raw agent address (only where it was accepted before: exec, ping, publish);
2. all digits → PID;
3. starts with `/`, `./`, `../`, `~`, or is `.`/`..` → path: a project dir or state dir, **exact**
   match; `--recursive` (list, stop, reboot, pause, resume) restores "every VM below";
4. otherwise `NAME` or `NAME/SERVICE`: a name from the index, else a dev state-dir basename.
   During the transition a bare word that is no name but an existing directory is taken as a
   path with a note on stderr; if it is both, the command fails and asks for `./word`.

No REF means the current directory, exact project match. `--service S` stays and is the same as
`REF/S`; giving both is an error.

### Runs: `--name`, managed dirs, recipe (S2, S5)

- `vk run --name X`: without `--state-dir`, the state dir is a managed
  `$XDG_STATE_HOME/virtkit/run/X-<8 hex>`. With `--state-dir D`, `X` names `D`.
  `X` already naming another dir is an error (`vk start X` or `vk rm X`).
- `vk run --detach` with neither `--name` nor `--state-dir` gets an automatic name and a managed
  dir, so every background VM is registered.
- Compose up (`--compose` with no image, `-f` or `--primary`) registers no VM, so it is never
  named: a name would list it stopped while it runs and `vk start` would boot a second
  fleet. `--name` on it is refused.
- `vk run --state-dir D` without `--name` gets an automatic name unless `D` has one.
- The CLI `vk run` path (not `vk dev`, not the executor) writes `<state dir>/run.json`: the
  argv after `run`, the working directory and the project directory. `vk start` replays it
  in that directory with `--detach --state-dir D --name N` appended before `--`
  (`Run` sets `args_override_self`, so the appended flags win) and the env
  `VIRTKIT_FROM_RECIPE=1`, which keeps the recipe from being rewritten.
- Registration also writes `<state dir>/.virtkit-last.json`, a copy of the registry entry, so a
  stopped VM can still be listed with its label, project and compose services.

### `vk list` (S3)

One row per VM: every running registered VM, every named VM that is stopped, and each compose
service of either (state from the control socket when the primary runs, `stopped` otherwise).

- Columns: `NAME` (the REF), `KIND` (`dev`, `run`, `service`), `STATUS`, `PID`, `UPTIME`, `MEM`,
  `IMAGE`, `PROJECT`, `PUBLISHED`; `--wide` adds `EXEC ADDRESS` and the full project path;
  `--stale` adds `STALE`.
- Order: grouped by project (dev: workspace; run: project dir), groups by path; within a group
  dev environments then runs, each primary followed by its services by name.
- `--running` / `--stopped` (conflicting) filter rows; `--kind dev|run|service`.
- `--json` is an array of these rows; `--field` selects from them.
- A REF selecting exactly one primary prints its detail record, as before.
- Unnamed running VMs (registered before this change) show their PID as NAME.
- `vk dev list` stays the per-environment storage view (sizes, flags, `gc` staleness); its
  NAME is the environment's name (dir basename when unnamed) and its JSON gains `dir_name`.

### Lifecycle verbs (S4)

| Verb | dev | run | service |
|---|---|---|---|
| `start` | `vk dev --workspace W --environment E up` (child process) | replay `run.json` | start the primary if stopped, then control-socket `Start` (dev: `vk dev service up`) |
| `stop` | `dev::stop` (as `vk dev stop`) | existing `stop_one` | control-socket `Stop` (dev: `vk dev service down`) |
| `restart` | stop, then start | same | same |
| `rm [-f]` | `dev::list::remove` (as `vk dev gc`) | delete a managed dir; for a user dir delete `run.json` and `.virtkit-last.json` only | delete `svc-<name>/` while stopped |
| `reset` | `vk dev prune` options | refused | refused |

`rm` refuses a running VM unless `-f`, which stops it first, and releases the name.
`vk dev gc NAME` and `vk dev prune NAME` keep working and also accept names; they print a note
pointing to `vk rm` / `vk reset`.

### Vocabulary (S6)

- `vk publish --service` is the option's name; `--via` stays as an alias.
- `vk ping [REF]` is the agent probe; `vk status` stays as an alias.
- `vk paths` calls the root the **data dir** and also lists the dev base, the run base, the
  name index and the `runs/` ledger.

### Hub (lockstep)

- `Workload` gains an optional `name`. vk fills it; the hub keys dev rows by `name`, falling
  back to the state-dir basename.
- `vk dev list --json` `name` is the environment name; the hub passes it to `vk dev stop/gc`,
  which accept it.

## Rejected alternatives

- **Names computed from paths at lookup time**: the first `app` would be renamed the moment a
  second `app` appears.
- **A name file inside each state dir as the source of truth**: a stopped user `--state-dir`
  could then not be found from its name without scanning the disk.
- **Name in `.virtkit/config.toml`**: committed, so every worktree would claim the same name.
- **`vk dev list` as an alias of `vk list --kind dev`**: it reports disk sizes and staleness
  that per-VM rows do not carry; it stays the environment storage view.

## Acceptance criteria

1. names: validate, claim with suffixes, lookup, dir → name, rename, release, dangling links
   (unit tests in `names.rs`).
2. REF parsing covers every case of the grammar (unit tests).
3. A path REF matches exactly; `--recursive` matches below (unit tests).
4. Dev dirs get names lazily, oldest first, ephemeral dirs skipped; basenames still resolve.
5. `vk run --name` / `--detach` / `--state-dir` naming and `run.json` contents (unit tests on
   the preparation function).
6. `vk list` rows: grouping, order, services, stopped VMs, filters (unit tests on the pure
   row builder).
7. start/stop/rm planning per kind (unit tests on the pure planner); `rm` never deletes an
   unmanaged dir.
8. `vk ping`, `vk status` alias, `publish --service`, `vk paths` lines (CLI parse tests).
9. vk-hub keys a dev environment by `Workload.name`, falling back to its state dir's own
   name, and the proto drops a `name` that is not one (unit tests in both crates).
10. `cargo build`, `cargo clippy`, `cargo test` for vk-driver, vk-hub, vk-hub-proto pass.

## Open questions

- Should unnamed foreground runs be registered under an automatic name later?
- A dev environment stopped before this change lists no compose services until its next
  boot writes `.virtkit-last.json`; reading them from `dev.json`'s compose manifest would fill
  the gap.
- A `vk run` that fails before its VM boots still leaves a named, stopped VM (as
  `docker create` would); `vk rm` removes it.
