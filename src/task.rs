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
