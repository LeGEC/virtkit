# Steps

1. **Name index** — `names.rs`: grammar, claim, lookup, reverse lookup, rename, release; `vk rename`.
2. **REF selector** — `Ref` parsing and resolution; exact path match with `--recursive`; wired into every VM command.
3. **Run names and recipe** — `vk run --name`, automatic names, managed run dirs, `run.json`, `.virtkit-last.json`.
4. **Dev names** — lazy naming of dev dirs, basename aliases, `vk dev list` NAME/`dir_name`, `dev stop/gc/prune` accept names.
5. **Unified `vk list`** — one row per VM, stopped VMs and services, grouping, filters, JSON.
6. **Lifecycle verbs** — `vk start | stop | restart | rm | reset`.
7. **Vocabulary** — `vk ping`, `publish --service`, `vk paths`.
8. **Hub lockstep** — `Workload.name`, hub dev keys.
9. **Docs** — README, docs/dev.md, e2e scripts.
