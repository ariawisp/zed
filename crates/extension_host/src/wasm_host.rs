pub mod wit;

use crate::capability_granter::CapabilityGranter;
use crate::extension_settings::ExtensionSettings;
use anyhow::{Context as _, Result, anyhow, bail};
use async_trait::async_trait;
use dap::{DebugRequest, StartDebuggingRequestArgumentsRequest};
use extension::{
    CodeLabel, Command, Completion, ContextServerConfiguration, DebugAdapterBinary,
    DebugTaskDefinition, ExtensionCapability, ExtensionHostProxy, KeyValueStoreDelegate,
    ProjectDelegate, SlashCommand, SlashCommandArgumentCompletion, SlashCommandOutput, Symbol,
    WorktreeDelegate, ExtensionManifest,
};
use fs::{Fs, normalize_path};
use futures::future::LocalBoxFuture;
use futures::{ Future, FutureExt, StreamExt as _, channel::{ mpsc::{self, UnboundedSender}, oneshot }, future::BoxFuture };
use gpui::{App, AsyncApp, BackgroundExecutor, Task, Timer};
use http_client::HttpClient;
use language::LanguageName;
use lsp::LanguageServerName;
use moka::sync::Cache;
use node_runtime::NodeRuntime;
use release_channel::ReleaseChannel;
use semantic_version::SemanticVersion;
use settings::Settings;
use std::borrow::Cow;
use std::sync::{LazyLock, OnceLock};
use std::time::Duration;
use std::{ path::{Path, PathBuf}, sync::Arc };
use task::{DebugScenario, SpawnInTerminal, TaskTemplate, ZedDebugConfig};
use util::paths::SanitizedPath;
use wasmtime::{ CacheStore, Engine, Store, component::{Component, ResourceTable} };
use wasmtime_runtime as generic_host;
use wasmtime_wasi::{self as wasi, WasiView};
use wit::Extension;

pub struct WasmHost {
    engine: Engine,
    release_channel: ReleaseChannel,
    http_client: Arc<dyn HttpClient>,
    node_runtime: NodeRuntime,
    pub(crate) proxy: Arc<ExtensionHostProxy>,
    fs: Arc<dyn Fs>,
    pub work_dir: PathBuf,
    pub(crate) granted_capabilities: Vec<ExtensionCapability>,
    _main_thread_message_task: Task<()>,
    main_thread_message_tx: mpsc::UnboundedSender<MainThreadCall>,
}

#[derive(Clone, Debug)]
pub struct WasmExtension {
    tx: UnboundedSender<ExtensionCall>,
    pub manifest: Arc<ExtensionManifest>,
    pub work_dir: Arc<Path>,
    #[allow(unused)]
    pub zed_api_version: SemanticVersion,
}

impl Drop for WasmExtension { fn drop(&mut self) { self.tx.close_channel(); } }

#[async_trait]
impl extension::Extension for WasmExtension {
    fn manifest(&self) -> Arc<ExtensionManifest> { self.manifest.clone() }
    fn work_dir(&self) -> Arc<Path> { self.work_dir.clone() }
    async fn language_server_command(&self, language_server_id: LanguageServerName, language_name: LanguageName, worktree: Arc<dyn WorktreeDelegate>) -> Result<Command> {
        self.call(|extension, store| { async move { let resource = store.data_mut().table().push(worktree)?; let command = extension.call_language_server_command(store, &language_server_id, &language_name, resource).await?.map_err(|err| store.data().extension_error(err))?; Ok(command.into()) }.boxed() }).await?
    }
    async fn language_server_initialization_options(&self, language_server_id: LanguageServerName, language_name: LanguageName, worktree: Arc<dyn WorktreeDelegate>) -> Result<Option<String>> { self.call(|extension, store| { async move { let resource = store.data_mut().table().push(worktree)?; let options = extension.call_language_server_initialization_options(store, &language_server_id, &language_name, resource).await?.map_err(|err| store.data().extension_error(err))?; anyhow::Ok(options) }.boxed() }).await? }
    async fn language_server_workspace_configuration(&self, language_server_id: LanguageServerName, worktree: Arc<dyn WorktreeDelegate>) -> Result<Option<String>> { self.call(|extension, store| { async move { let resource = store.data_mut().table().push(worktree)?; let options = extension.call_language_server_workspace_configuration(store, &language_server_id, resource).await?.map_err(|err| store.data().extension_error(err))?; anyhow::Ok(options) }.boxed() }).await? }
    async fn language_server_additional_initialization_options(&self, language_server_id: LanguageServerName, target_language_server_id: LanguageServerName, worktree: Arc<dyn WorktreeDelegate>) -> Result<Option<String>> { self.call(|extension, store| { async move { let resource = store.data_mut().table().push(worktree)?; let options = extension.call_language_server_additional_initialization_options(store, &language_server_id, &target_language_server_id, resource).await?.map_err(|err| store.data().extension_error(err))?; anyhow::Ok(options) }.boxed() }).await? }
    async fn language_server_additional_workspace_configuration(&self, language_server_id: LanguageServerName, target_language_server_id: LanguageServerName, worktree: Arc<dyn WorktreeDelegate>) -> Result<Option<String>> { self.call(|extension, store| { async move { let resource = store.data_mut().table().push(worktree)?; let options = extension.call_language_server_additional_workspace_configuration(store, &language_server_id, &target_language_server_id, resource).await?.map_err(|err| store.data().extension_error(err))?; anyhow::Ok(options) }.boxed() }).await? }
    async fn labels_for_completions(&self, language_server_id: LanguageServerName, completions: Vec<Completion>) -> Result<Vec<Option<CodeLabel>>> { self.call(|extension, store| { async move { let labels = extension.call_labels_for_completions(store, &language_server_id, completions.into_iter().map(Into::into).collect()).await?.map_err(|err| store.data().extension_error(err))?; Ok(labels.into_iter().map(|label| label.map(Into::into)).collect()) }.boxed() }).await? }
    async fn labels_for_symbols(&self, language_server_id: LanguageServerName, symbols: Vec<Symbol>) -> Result<Vec<Option<CodeLabel>>> { self.call(|extension, store| { async move { let labels = extension.call_labels_for_symbols(store, &language_server_id, symbols.into_iter().map(Into::into).collect()).await?.map_err(|err| store.data().extension_error(err))?; Ok(labels.into_iter().map(|label| label.map(Into::into)).collect()) }.boxed() }).await? }
    async fn complete_slash_command_argument(&self, command: SlashCommand, arguments: Vec<String>) -> Result<Vec<SlashCommandArgumentCompletion>> { self.call(|extension, store| { async move { let completions = extension.call_complete_slash_command_argument(store, &command.into(), &arguments).await?.map_err(|err| store.data().extension_error(err))?; Ok(completions.into_iter().map(Into::into).collect()) }.boxed() }).await? }
    async fn run_slash_command(&self, command: SlashCommand, arguments: Vec<String>, delegate: Option<Arc<dyn WorktreeDelegate>>) -> Result<SlashCommandOutput> { self.call(|extension, store| { async move { let resource = if let Some(delegate) = delegate { Some(store.data_mut().table().push(delegate)?) } else { None }; let output = extension.call_run_slash_command(store, &command.into(), &arguments, resource).await?.map_err(|err| store.data().extension_error(err))?; Ok(output.into()) }.boxed() }).await? }
    async fn context_server_command(&self, context_server_id: Arc<str>, project: Arc<dyn ProjectDelegate>) -> Result<Command> { self.call(|extension, store| { async move { let project_resource = store.data_mut().table().push(project)?; let command = extension.call_context_server_command(store, context_server_id.clone(), project_resource).await?.map_err(|err| store.data().extension_error(err))?; anyhow::Ok(command.into()) }.boxed() }).await? }
    async fn context_server_configuration(&self, context_server_id: Arc<str>, project: Arc<dyn ProjectDelegate>) -> Result<Option<ContextServerConfiguration>> { self.call(|extension, store| { async move { let project_resource = store.data_mut().table().push(project)?; let Some(configuration) = extension.call_context_server_configuration(store, context_server_id.clone(), project_resource).await?.map_err(|err| store.data().extension_error(err))? else { return Ok(None) }; Ok(Some(configuration)) }.boxed() }).await? }
    async fn key_value_store_open(&self, name: String, value: Arc<dyn KeyValueStoreDelegate>) -> Result<()> { self.call(|extension, store| { async move { let resource = store.data_mut().table().push(value)?; extension.call_key_value_store_open(store, name.clone(), resource).await?; Ok(()) }.boxed() }).await }
    async fn run_debug_task(&self, _debug_adapter_binary: Option<DebugAdapterBinary>, task: DebugTaskDefinition, template: TaskTemplate, scenario: DebugScenario) -> Result<StartDebuggingRequestArgumentsRequest> { self.call(|extension, store| { async move { let request = extension.call_run_debug_task(store, task, template, scenario).await?.map_err(|err| store.data().extension_error(err))?; Ok(request) }.boxed() }).await? }
}

pub struct WasmState {
    manifest: Arc<ExtensionManifest>,
    pub table: ResourceTable,
    ctx: wasi::WasiCtx,
    pub host: Arc<WasmHost>,
    pub(crate) capability_granter: CapabilityGranter,
}

type MainThreadCall = Box<dyn Send + for<'a> FnOnce(&'a mut AsyncApp) -> LocalBoxFuture<'a, ()>>;
type ExtensionCall = Box<dyn Send + for<'a> FnOnce(&'a mut Extension, &'a mut Store<WasmState>) -> BoxFuture<'a, ()>>;

fn wasm_engine(executor: &BackgroundExecutor) -> wasmtime::Engine { static WASM_ENGINE: OnceLock<wasmtime::Engine> = OnceLock::new(); WASM_ENGINE.get_or_init(|| { let engine = generic_host::new_engine(generic_host::EngineOptions { component_model: true, async_support: true, epoch_interruption: true, incremental_cache: true, parallel_compilation: true }).unwrap(); let engine_ref = engine.weak(); executor.spawn(async move { const EPOCH_INTERVAL: Duration = Duration::from_millis(100); let mut timer = Timer::interval(EPOCH_INTERVAL); while (timer.next().await).is_some() { if let Some(engine) = engine_ref.upgrade() { engine.increment_epoch(); } else { break; } } }).detach(); engine }).clone() }

fn cache_store() -> Arc<IncrementalCompilationCache> { static CACHE_STORE: LazyLock<Arc<IncrementalCompilationCache>> = LazyLock::new(|| Arc::new(IncrementalCompilationCache::new())); CACHE_STORE.clone() }

impl WasmHost { pub fn new(fs: Arc<dyn Fs>, http_client: Arc<dyn HttpClient>, node_runtime: NodeRuntime, proxy: Arc<ExtensionHostProxy>, work_dir: PathBuf, cx: &mut App) -> Arc<Self> { let (tx, mut rx) = mpsc::unbounded::<MainThreadCall>(); let task = cx.spawn(async move |cx| { while let Some(message) = rx.next().await { message(cx).await; } }); let extension_settings = ExtensionSettings::get_global(cx); Arc::new(Self { engine: wasm_engine(cx.background_executor()), fs, work_dir, http_client, node_runtime, proxy, release_channel: ReleaseChannel::global(cx), granted_capabilities: extension_settings.granted_capabilities.clone(), _main_thread_message_task: task, main_thread_message_tx: tx }) } }

pub fn parse_wasm_extension_version(extension_id: &str, wasm_bytes: &[u8]) -> Result<SemanticVersion> { let mut version = None; for part in wasmparser::Parser::new(0).parse_all(wasm_bytes) { if let wasmparser::Payload::CustomSection(s) = part.context("error parsing wasm extension")? && s.name() == "zed:api-version" { version = parse_wasm_extension_version_custom_section(s.data()); if version.is_none() { bail!("extension {} has invalid zed:api-version section: {:?}", extension_id, s.data()); } } } version.with_context(|| format!("extension {extension_id} has no zed:api-version section")) }

fn parse_wasm_extension_version_custom_section(data: &[u8]) -> Option<SemanticVersion> { if data.len() == 6 { Some(SemanticVersion::new(u16::from_be_bytes([data[0], data[1]]) as _, u16::from_be_bytes([data[2], data[3]]) as _, u16::from_be_bytes([data[4], data[5]]) as _)) } else { None } }

impl wasi::WasiView for WasmState { fn table(&mut self) -> &mut ResourceTable { &mut self.table } fn ctx(&mut self) -> &mut wasi::WasiCtx { &mut self.ctx } }

#[derive(Debug)] struct IncrementalCompilationCache { cache: Cache<Vec<u8>, Vec<u8>> }
impl IncrementalCompilationCache { fn new() -> Self { let cache = Cache::builder().max_capacity(32 * 1024 * 1024).weigher(|k: &Vec<u8>, v: &Vec<u8>| (k.len() + v.len()).try_into().unwrap_or(u32::MAX)).build(); Self { cache } } }
impl CacheStore for IncrementalCompilationCache { fn get(&self, key: &[u8]) -> Option<Cow<'_, [u8]>> { self.cache.get(key).map(|v| v.into()) } fn insert(&self, key: &[u8], value: Vec<u8>) -> bool { self.cache.insert(key.to_vec(), value); true } }
