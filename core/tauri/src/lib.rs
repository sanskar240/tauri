// Copyright 2019-2021 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

//! Tauri is a framework for building tiny, blazing fast binaries for all major desktop platforms.
//! Developers can integrate any front-end framework that compiles to HTML, JS and CSS for building their user interface.
//! The backend of the application is a rust-sourced binary with an API that the front-end can interact with.
//!
//! The user interface in Tauri apps currently leverages Cocoa/WebKit on macOS, gtk-webkit2 on Linux and MSHTML (IE10/11) or Webkit via Edge on Windows.
//! Tauri uses (and contributes to) the MIT licensed project that you can find at [webview](https://github.com/webview/webview).
#![warn(missing_docs, rust_2018_idioms)]

/// The Tauri error enum.
pub use error::Error;
pub use tauri_macros::{command, generate_handler};

/// Core API.
pub mod api;
/// Async runtime.
pub mod async_runtime;
pub mod command;
/// The Tauri API endpoints.
mod endpoints;
mod error;
mod event;
mod hooks;
pub mod plugin;
pub mod runtime;
/// The Tauri-specific settings for your runtime e.g. notification permission status.
pub mod settings;
mod state;
#[cfg(feature = "updater")]
pub mod updater;

/// `Result<T, ::tauri::Error>`
pub type Result<T> = std::result::Result<T, Error>;

/// A task to run on the main thread.
pub type SyncTask = Box<dyn FnOnce() + Send>;

use crate::{
  event::{Event, EventHandler},
  runtime::{
    tag::{Tag, TagRef},
    window::PendingWindow,
    Dispatch, Runtime,
  },
};
use serde::Serialize;
use std::{borrow::Borrow, collections::HashMap, path::PathBuf, sync::Arc};

// Export types likely to be used by the application.
pub use {
  self::api::{
    assets::Assets,
    config::{Config, WindowUrl},
  },
  self::hooks::{
    Invoke, InvokeError, InvokeHandler, InvokeMessage, InvokeResolver, InvokeResponse, OnPageLoad,
    PageLoadPayload, SetupHook,
  },
  self::runtime::app::{App, Builder},
  self::runtime::flavors::wry::Wry,
  self::runtime::webview::{WebviewAttributes, WindowBuilder},
  self::runtime::window::export::Window,
  self::state::{State, StateManager},
};

/// Reads the config file at compile time and generates a [`Context`] based on its content.
///
/// The default config file path is a `tauri.conf.json` file inside the Cargo manifest directory of
/// the crate being built.
///
/// # Custom Config Path
///
/// You may pass a string literal to this macro to specify a custom path for the Tauri config file.
/// If the path is relative, it will be search for relative to the Cargo manifest of the compiling
/// crate.
///
/// # Note
///
/// This macro should not be called if you are using [`tauri-build`] to generate the context from
/// inside your build script as it will just cause excess computations that will be discarded. Use
/// either the [`tauri-build] method or this macro - not both.
///
/// [`tauri-build`]: https://docs.rs/tauri-build
pub use tauri_macros::generate_context;

/// Include a [`Context`] that was generated by [`tauri-build`] inside your build script.
///
/// You should either use [`tauri-build`] and this macro to include the compile time generated code,
/// or [`generate_context!`]. Do not use both at the same time, as they generate the same code and
/// will cause excess computations that will be discarded.
///
/// [`tauri-build`]: https://docs.rs/tauri-build
#[macro_export]
macro_rules! tauri_build_context {
  () => {
    include!(concat!(env!("OUT_DIR"), "/tauri-build-context.rs"))
  };
}

/// A icon definition.
#[derive(Debug, Clone)]
pub enum Icon {
  /// Icon from file path.
  File(PathBuf),
  /// Icon from raw bytes.
  Raw(Vec<u8>),
}

/// User supplied data required inside of a Tauri application.
pub struct Context<A: Assets> {
  /// The config the application was prepared with.
  pub config: Config,

  /// The assets to be served directly by Tauri.
  pub assets: Arc<A>,

  /// The default window icon Tauri should use when creating windows.
  pub default_window_icon: Option<Vec<u8>>,

  /// Package information.
  pub package_info: crate::api::PackageInfo,
}

/// Types associated with the running Tauri application.
pub trait Params: sealed::ParamsBase {
  /// The event type used to create and listen to events.
  type Event: Tag;

  /// The type used to determine the name of windows.
  type Label: Tag;

  /// Assets that Tauri should serve from itself.
  type Assets: Assets;

  /// The underlying webview runtime used by the Tauri application.
  type Runtime: Runtime;
}

/// Manages a running application.
///
/// TODO: expand these docs
pub trait Manager<P: Params>: sealed::ManagerBase<P> {
  /// The [`Config`] the manager was created with.
  fn config(&self) -> Arc<Config> {
    self.manager().config()
  }

  /// Emits a event to all windows.
  fn emit_all<E: ?Sized, S>(&self, event: &E, payload: Option<S>) -> Result<()>
  where
    P::Event: Borrow<E>,
    E: TagRef<P::Event>,
    S: Serialize + Clone,
  {
    self.manager().emit_filter(event, payload, |_| true)
  }

  /// Emits an event to a window with the specified label.
  fn emit_to<E: ?Sized, L: ?Sized, S: Serialize + Clone>(
    &self,
    label: &L,
    event: &E,
    payload: Option<S>,
  ) -> Result<()>
  where
    P::Label: Borrow<L>,
    P::Event: Borrow<E>,
    L: TagRef<P::Label>,
    E: TagRef<P::Event>,
  {
    self
      .manager()
      .emit_filter(event, payload, |w| label == w.label())
  }

  /// Creates a new [`Window`] on the [`Runtime`] and attaches it to the [`Manager`].
  fn create_window(&mut self, pending: PendingWindow<P>) -> Result<Window<P>> {
    use sealed::RuntimeOrDispatch::*;

    let labels = self.manager().labels().into_iter().collect::<Vec<_>>();
    let pending = self.manager().prepare_window(pending, &labels)?;
    match self.runtime() {
      Runtime(runtime) => runtime.create_window(pending),
      Dispatch(mut dispatcher) => dispatcher.create_window(pending),
    }
    .map(|window| self.manager().attach_window(window))
  }

  /// Listen to a global event.
  fn listen_global<E: Into<P::Event>, F>(&self, event: E, handler: F) -> EventHandler
  where
    F: Fn(Event) + Send + 'static,
  {
    self.manager().listen(event.into(), None, handler)
  }

  /// Listen to a global event only once.
  fn once_global<E: Into<P::Event>, F>(&self, event: E, handler: F) -> EventHandler
  where
    F: Fn(Event) + Send + 'static,
  {
    self.manager().once(event.into(), None, handler)
  }

  /// Trigger a global event.
  fn trigger_global<E: ?Sized>(&self, event: &E, data: Option<String>)
  where
    P::Event: Borrow<E>,
    E: TagRef<P::Event>,
  {
    self.manager().trigger(event, None, data)
  }

  /// Remove an event listener.
  fn unlisten(&self, handler_id: EventHandler) {
    self.manager().unlisten(handler_id)
  }

  /// Fetch a single window from the manager.
  fn get_window<L: ?Sized>(&self, label: &L) -> Option<Window<P>>
  where
    P::Label: Borrow<L>,
    L: TagRef<P::Label>,
  {
    self.manager().get_window(label)
  }

  /// Fetch all managed windows.
  fn windows(&self) -> HashMap<P::Label, Window<P>> {
    self.manager().windows()
  }

  /// Add `state` to the state managed by the application.
  /// See [`tauri::Builder#manage`] for instructions.
  fn manage<T>(&self, state: T)
  where
    T: Send + Sync + 'static,
  {
    self.manager().state().set(state);
  }

  /// Gets the managed state for the type `T`.
  fn state<T>(&self) -> State<'_, T>
  where
    T: Send + Sync + 'static,
  {
    self.manager().inner.state.get()
  }
}

/// Prevent implementation details from leaking out of the [`Manager`] and [`Params`] traits.
pub(crate) mod sealed {
  use super::Params;
  use crate::runtime::{manager::WindowManager, Runtime};

  /// No downstream implementations of [`Params`].
  pub trait ParamsBase: 'static {}

  /// A running [`Runtime`] or a dispatcher to it.
  pub enum RuntimeOrDispatch<'r, P: Params> {
    /// Mutable reference to the running [`Runtime`].
    Runtime(&'r mut P::Runtime),

    /// A dispatcher to the running [`Runtime`].
    Dispatch(<P::Runtime as Runtime>::Dispatcher),
  }

  /// Managed handle to the application runtime.
  pub trait ManagerBase<P: Params> {
    /// The manager behind the [`Managed`] item.
    fn manager(&self) -> &WindowManager<P>;

    /// The runtime or runtime dispatcher of the [`Managed`] item.
    fn runtime(&mut self) -> RuntimeOrDispatch<'_, P>;
  }
}

#[cfg(test)]
mod test {
  use proptest::prelude::*;

  proptest! {
    #![proptest_config(ProptestConfig::with_cases(10000))]
    #[test]
    // check to see if spawn executes a function.
    fn check_spawn_task(task in "[a-z]+") {
      // create dummy task function
      let dummy_task = async move {
        format!("{}-run-dummy-task", task);
      };
      // call spawn
      crate::async_runtime::spawn(dummy_task);
    }
  }
}
