//! Named command-line tasks.

use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;

use crate::{AppContext, Error, Result};

/// A task's name and description.
#[derive(Debug, Clone)]
pub struct TaskInfo {
    /// The name used to run it.
    pub name: String,
    /// One-line description.
    pub detail: String,
}

/// `key:value` arguments passed to a task.
#[derive(Debug, Clone, Default)]
pub struct Vars {
    /// The arguments.
    pub cli: BTreeMap<String, String>,
}

impl Vars {
    /// Build from parsed pairs.
    #[must_use]
    pub fn from_cli_args(args: Vec<(String, String)>) -> Self {
        Self {
            cli: args.into_iter().collect(),
        }
    }

    /// Get a required argument.
    ///
    /// # Errors
    ///
    /// Fails when the argument is missing.
    pub fn cli_arg(&self, key: &str) -> Result<&String> {
        self.cli
            .get(key)
            .ok_or_else(|| Error::Message(format!("missing task argument: {key}")))
    }
}

/// A runnable task.
#[async_trait]
pub trait Task: Send + Sync {
    /// The task's name and description.
    fn task(&self) -> TaskInfo;
    /// Run it.
    async fn run(&self, ctx: &AppContext, vars: &Vars) -> Result<()>;
}

/// The tasks a service offers.
#[derive(Clone, Default)]
pub struct Tasks {
    registered: BTreeMap<String, Arc<dyn Task>>,
}

impl Tasks {
    /// Add a task.
    pub fn register(&mut self, task: impl Task + 'static) {
        self.registered.insert(task.task().name, Arc::new(task));
    }

    /// Names and descriptions, sorted by name.
    #[must_use]
    pub fn list(&self) -> Vec<TaskInfo> {
        self.registered.values().map(|task| task.task()).collect()
    }

    /// Run a task by name.
    ///
    /// # Errors
    ///
    /// Fails when the name is unknown or the task fails.
    pub async fn run(&self, name: &str, ctx: &AppContext, vars: &Vars) -> Result<()> {
        let task = self
            .registered
            .get(name)
            .ok_or_else(|| Error::Message(format!("unknown task: {name}")))?;
        task.run(ctx, vars).await
    }
}

#[cfg(all(test, not(feature = "sea-orm")))]
mod tests {
    use super::*;
    use crate::{
        Environment,
        config::{Config, Defaults},
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RUNS: AtomicUsize = AtomicUsize::new(0);

    struct Count;

    #[async_trait]
    impl Task for Count {
        fn task(&self) -> TaskInfo {
            TaskInfo {
                name: "count".to_string(),
                detail: "Counts".to_string(),
            }
        }

        async fn run(&self, _ctx: &AppContext, vars: &Vars) -> Result<()> {
            let by: usize = vars.cli_arg("by")?.parse().map_err(Error::wrap)?;
            RUNS.fetch_add(by, Ordering::SeqCst);
            Ok(())
        }
    }

    fn context() -> AppContext {
        let defaults = Defaults {
            database: "",
            ..Defaults::default()
        };
        let config = Config::from_lookup(&Environment::Test, &defaults, |_| None).unwrap();
        AppContext::for_test(Environment::Test, config)
    }

    #[tokio::test]
    async fn a_registered_task_runs_with_its_arguments() {
        let mut tasks = Tasks::default();
        tasks.register(Count);
        let vars = Vars::from_cli_args(vec![("by".to_string(), "2".to_string())]);
        tasks.run("count", &context(), &vars).await.unwrap();
        assert_eq!(RUNS.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_unknown_task_is_an_error() {
        let error = Tasks::default()
            .run("nope", &context(), &Vars::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("unknown task"));
    }

    #[tokio::test]
    async fn a_missing_argument_is_an_error() {
        let mut tasks = Tasks::default();
        tasks.register(Count);
        let error = tasks
            .run("count", &context(), &Vars::default())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("missing task argument: by"));
    }

    #[test]
    fn tasks_list_their_names() {
        let mut tasks = Tasks::default();
        tasks.register(Count);
        assert_eq!(tasks.list()[0].name, "count");
    }
}
