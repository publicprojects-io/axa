# Upgrading to Axa 0.24

This covers a service on 0.23. Nothing in 0.24 removes or changes what 0.23 had, so a
service upgrades by changing the versions. That holds for a service with a database and
for one without.

## What will not compile

| Change | What to do |
|---|---|
| `axa-test` 0.15 is the one for `axa` 0.24 | Set `axa-test = "0.15"` in `[dev-dependencies]` |

## What is new

| Addition | Use |
|---|---|
| `Cli::environment()` | In place of `cli.environment.as_deref().map_or_else(Environment::from_env, Environment::parse)` |
| `cli::answer(&cli, &environment, &defaults)` | Answers `check` and `health` before anything opens, in place of matching them by hand. `None` for the other commands |
| `cli.config(&environment, &defaults)` | `Config::for_task` for a task and `Config::from_env` otherwise, with `apply_cli` done, in place of choosing and applying by hand |
| `Tasks::run_or_list(name.as_deref(), &ctx, &params)` | Answers `task`: lists the tasks when no name is given, in place of the loop that printed them and `parse_vars` |
| `Settings::retired(old, new)` and `Settings::refuse(sentence)` (in 0.23.1) | A service's own old setting name, or any problem, refused in every environment, where `renamed` refuses only in production |

A hand-written `run` shrinks to this:

```rust,ignore
pub async fn run(cli: Cli) -> axa::Result<()> {
    let environment = cli.environment();
    if let Some(answered) = axa::cli::answer(&cli, &environment, &defaults()).await {
        return answered;
    }
    let config = cli.config(&environment, &defaults())?;
    axa::logger::init(env!("CARGO_CRATE_NAME"), &config.logger);
    let ctx = AppContext::new(environment, (), config);
    match cli.command {
        Command::Start { .. } => axa::server::serve(router(&ctx)?, &ctx.config.server).await,
        Command::Task { name, params } => tasks().run_or_list(name.as_deref(), &ctx, &params).await,
        Command::Db { .. } => Err(axa::Error::string("this service has no database")),
        Command::Check | Command::Health { .. } => Ok(()), // Answered above.
    }
}
```
