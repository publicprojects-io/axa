//! Common imports for handlers.

pub use async_trait::async_trait;
pub use axum::{
    Json, debug_handler,
    extract::{Form, Multipart, Path, Query, State},
    response::{IntoResponse, Response},
    routing::{delete, get, head, options, patch, post, put, trace},
};
#[cfg(has_seaorm)]
pub use sea_orm::prelude::{Date, DateTimeUtc, DateTimeWithTimeZone, Decimal, Uuid};
#[cfg(has_seaorm)]
pub use sea_orm::{
    ActiveModelBehavior, ActiveModelTrait, ActiveValue, ColumnTrait, Condition, ConnectionTrait,
    DatabaseConnection, DbErr, EntityTrait, IntoActiveModel, ModelTrait, Order, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, TransactionTrait,
};

#[cfg(has_seaorm)]
pub use crate::query;
pub use crate::{
    AppContext, Error, Result,
    error::{ErrorDetail, bad_request, not_found, unauthorized},
    format,
    routes::{AppRoutes, Routes},
    store::SharedStore,
    task::{self, Task, TaskInfo},
    worker::BackgroundWorker,
};
#[cfg(feature = "model")]
pub use crate::{
    model::{ModelError, ModelResult},
    validation::Validatable,
};
