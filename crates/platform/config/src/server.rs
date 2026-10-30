//! Server operational configuration.
//!
//! `orch.toml`: what an operator decides for a deployment. Every key has
//! the default that ships; the comment on each says what it is for. Mail
//! servers, bots and everything else people set up in the product are in
//! the database, not here; secrets are files of their own.

use crate::loader::{ConfigError, Validate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

fn s(text: &str) -> String {
    text.to_owned()
}

fn list(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

macro_rules! section {
    ($(#[$meta:meta])* $name:ident { $( $(#[$fmeta:meta])* $field:ident : $ty:ty = $default:expr ),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty, )*
        }

        impl Default for $name {
            fn default() -> Self {
                Self { $( $field: $default, )* }
            }
        }
    };
}

section! {
    /// The whole of `orch.toml`.
    ServerConfig {
        /// The version of this file's format.
        version: u32 = 1,
        /// The system's name and address.
        system: System = System::default(),
        /// Stable browser names for owned environment ports.
        edge: Edge = Edge::default(),
        /// The API service.
        api: Api = Api::default(),
        /// The gateway that machines connect to.
        gateway: Gateway = Gateway::default(),
        /// The link between machines and the gateway.
        link: Link = Link::default(),
        /// The worker.
        worker: Worker = Worker::default(),
        /// The database.
        database: Database = Database::default(),
        /// Where content is kept.
        storage: Storage = Storage::default(),
        /// Sessions and signing in.
        auth: Auth = Auth::default(),
        /// The certificate authority.
        pki: Pki = Pki::default(),
        /// The bundled environment host.
        node: Node = Node::default(),
        /// A host node's own ceiling for host ports, which comes before every declaration of the
        /// host's page.
        ports: Ports = Ports::default(),
        /// External implementations of extension points.
        plugin: Vec<Plugin> = Vec::new(),
        /// The thread engine.
        engine: Engine = Engine::default(),
        /// How long things are kept.
        retention: Retention = Retention::default(),
        /// Stored measurements.
        metrics: Metrics = Metrics::default(),
        /// The operator's read-only release source and request bounds.
        release: Release = Release::default(),
        /// Protected metadata written by the installation's existing updater/backup command.
        installation: Installation = Installation::default(),
        /// Bounds an operator may move.
        limits: Limits = Limits::default(),
        /// Document history.
        documents: Documents = Documents::default(),
        /// Browser connections.
        realtime: Realtime = Realtime::default(),
        /// The interval of each scheduled job, in seconds, by the job's name;
        /// a job that is not named keeps its own interval.
        schedule: BTreeMap<String, u64> = BTreeMap::new(),
        /// Log output.
        log: Log = Log::default(),
    }
}

section! {
    /// `[system]`
    System {
        /// What the system calls itself.
        name: String = s("Orch"),
        /// The origin a browser opens; used in links and mail.
        public_url: String = s("https://orch.example.com"),
    }
}

section! {
    /// `[api]`
    Api {
        /// The browser API, TLS under the system's own CA, reached through the front proxy.
        bind: String = s("0.0.0.0:8443"),
        /// Mutual TLS: the gateway's backplane and certificate renewal.
        internal_bind: String = s("0.0.0.0:8444"),
        /// Plain HTTP for an operator's collector, on the internal network only.
        metrics_bind: String = s("0.0.0.0:9100"),
        /// Database connections. Thirty-two serve a few hundred people at
        /// once with room for the engine's own work.
        db_pool: u32 = 32,
        /// Proxy ranges allowed to describe caller addresses for the API and gateway edge.
        /// Empty trusts only the socket peer; a deployment names its proxy network.
        trusted_proxy_cidrs: Vec<String> = Vec::new(),
        /// How long an idempotency key replays its first answer.
        idempotency_ttl_hours: u64 = 24,
    }
}

section! {
    /// `[release]`: the same source `update.sh` reads through ORCH_RELEASE_URL.
    Release {
        /// Base release address; the repository's GitHub releases unless the operator overrides it.
        url: String = format!("{}/releases", env!("CARGO_PKG_REPOSITORY")),
        /// Total time for the one small manifest lookup.
        request_timeout_secs: u64 = 10,
        /// Largest accepted manifest: one MiB holds the ordinary release's asset list.
        manifest_max_bytes: usize = 1_048_576,
    }
}

impl Release {
    /// The same effective base for the server lookup and the host updater's release-source command.
    /// Empty legacy configuration selects the repository default, just like an omitted member.
    #[must_use]
    pub fn source(&self) -> String {
        if self.url.trim().is_empty() {
            format!("{}/releases", env!("CARGO_PKG_REPOSITORY"))
        } else {
            self.url.trim().trim_end_matches('/').to_owned()
        }
    }
}

section! {
    /// `[installation]`: local protected operator metadata, mounted read-only into the API.
    Installation {
        /// The directory containing latest-database-backup.json and its named manifest copy.
        records_dir: PathBuf = PathBuf::from("/var/lib/orch/installation"),
    }
}

section! {
    /// `[edge]`
    Edge {
        /// A configured wildcard browser domain; absent leaves only ordinary helper entry.
        ports_domain: Option<String> = None,
    }
}

section! {
    /// One address of the gateway, as it is advertised to machines.
    GatewayEndpoint {
        /// The address.
        url: String = String::new(),
        /// How a machine verifies it: `pinned` (the system's CA and a client
        /// certificate) or `public` (public roots and a signed handshake).
        trust: String = s("pinned"),
    }
}

section! {
    /// `[gateway]`
    Gateway {
        /// Machines that present client certificates.
        pinned_bind: String = s("[::]:9443"),
        /// Machines that come through the public front; TLS under the system's CA.
        edge_bind: String = s("0.0.0.0:9444"),
        /// Plain HTTP for an operator's collector.
        metrics_bind: String = s("0.0.0.0:9101"),
        /// Where the gateway reaches the API.
        backplane_url: String = s("wss://api:8444/internal/backplane"),
        /// This gateway's name among several.
        gateway_id: String = s("compose-gateway"),
        /// The address given to runtimes the system creates itself.
        managed_endpoint: String = String::new(),
        /// The addresses advertised to connected machines, in order of preference.
        endpoints: Vec<GatewayEndpoint> = vec![
            GatewayEndpoint { url: s("wss://gateway.example.com:9443"), trust: s("pinned") },
        ],
    }
}

section! {
    /// `[link]`
    Link {
        /// How often each side says it is alive.
        heartbeat_secs: u64 = 20,
        /// Silence after which the other side is taken for gone: three heartbeats.
        dead_after_secs: u64 = 60,
        /// The largest frame.
        max_frame_bytes: u64 = 262_144,
        /// How much of a tunnel may be on its way unacknowledged.
        tunnel_window_bytes: u64 = 1_048_576,
        /// How long a tunnel whose session dropped may be taken up again.
        tunnel_resume_grace_secs: u64 = 60,
        /// How much of a tunnel is kept for that.
        tunnel_replay_buffer_bytes: u64 = 8_388_608,
        /// Live TCP connections carried to one environment's ports at once.
        port_connections_max: u64 = 64,
        /// Accepted TCP connections to a port waiting for their open result.
        port_waiting_max: u64 = 16,
        /// The credit and the replay of one TCP direction of a port, in bytes.
        port_buffer_bytes: u64 = 262_144,
        /// The silence that ends a UDP sender's association with a port.
        port_udp_idle_secs: u64 = 60,
        /// UDP sender associations of one environment.
        port_udp_senders_max: u64 = 128,
        /// The largest UDP datagram carried, in bytes; a frame carries no more than 4 000.
        port_udp_datagram_bytes: u64 = 4_000,
        /// UDP payload waiting for one environment, in bytes.
        port_udp_buffer_bytes: u64 = 262_144,
        /// Ended port connections kept per opening or own-use port.
        port_recent_connections: u64 = 128,
        /// Ended port connections kept per environment, by its gateway and its node together.
        port_recent_total: u64 = 2_048,
        /// How long an ended port connection is kept, in memory only.
        port_recent_secs: u64 = 900,
        /// Distinct caller groups of a port per environment, by its gateway and its node together.
        port_caller_totals: u64 = 128,
    }
}

section! {
    /// `[worker]`
    Worker {
        /// Plain HTTP for an operator's collector.
        metrics_bind: String = s("0.0.0.0:9102"),
        /// Database connections.
        db_pool: u32 = 8,
        /// The pause before each further try of a delivery that failed.
        outbox_retry_secs: Vec<u64> = vec![60, 300, 900, 3_600, 21_600],
    }
}

section! {
    /// `[database]`
    Database {
        /// The address of the database, with its user and password. Given
        /// by `ORCH_DATABASE_URL` in a deployment, so that the password is
        /// in no file that is read by more than one service.
        url: String = String::new(),
        /// Where the schema's migrations are.
        migrations_dir: String = s("/opt/orch/migrations"),
        /// Log a statement whose elapsed time reaches this many milliseconds, without its bound values.
        slow_statement_ms: u64 = 1_000,
        /// Log a successful pooled connection acquisition taking more than this many milliseconds.
        slow_acquire_ms: u64 = 1_000,
    }
}

section! {
    /// `[storage]`
    Storage {
        /// The directory of `blobs/`, `uploads/` and `tmp/`.
        root: String = s("/var/lib/orch"),
    }
}

impl Storage {
    /// The directory of scratch files below the root (`tmp/`): the unfinished writes of the store, and the zips of
    /// archives while they are written.
    #[must_use]
    pub fn scratch_dir(&self) -> std::path::PathBuf {
        std::path::Path::new(&self.root).join("tmp")
    }
}

section! {
    /// `[auth]`
    Auth {
        /// The life of an access token. Fifteen minutes: what a stolen one is worth.
        access_token_ttl_secs: u64 = 900,
        /// How long a session lives after it was last used.
        session_ttl_days: u64 = 30,
        /// The shortest password.
        password_min_length: u32 = 10,
        /// Wrong tries of one proof at one name, wherever they come from…
        login_failure_limit: u32 = 8,
        /// …in any three hours; then that proof is refused there until the
        /// oldest is that old. An honest person has eight tries; a guesser
        /// has sixty-four a day.
        login_failure_window_secs: u64 = 10_800,
        /// Wrong tries from one address, whatever the names…
        address_failure_limit: u32 = 30,
        /// …in fifteen minutes: against one machine trying name after name.
        address_failure_window_secs: u64 = 900,
        /// The life of a mailed sign-in code: long enough for mail to arrive.
        email_code_ttl_secs: u64 = 600,
        /// Codes mailed to one name in the same three hours, at least a minute apart.
        email_code_send_limit: u32 = 5,
        /// How long a browser stays known after its person last got in
        /// from it: three lives of a session.
        known_device_days: u64 = 90,
    }
}

section! {
    /// `[pki]`
    Pki {
        /// The validity of a certificate issued to a service or a machine.
        leaf_validity_days: u64 = 365,
        /// How long after it expired a certificate may still be renewed.
        expired_renew_grace_days: u64 = 90,
        /// The names in the services' certificates.
        service_dns_names: Vec<String> = list(&["api", "gateway", "worker", "gateway.orch.internal"]),
        /// The validity of a newly created CA.
        ca_validity_days: u64 = 3_650,
        /// The life of an enrollment token.
        enrollment_token_ttl_secs: u64 = 900,
        /// How long before expiry a machine's owner is reminded.
        renewal_reminder_days: u64 = 14,
    }
}

section! {
    /// One registry whose login is loaded only by the node from a protected JSON file.
    NodeDockerRegistry {
        /// Registry authority as it appears in an image reference, including a port when needed.
        server: String = String::new(),
        /// File containing username/password or identity_token; relative to the configuration file.
        credentials_file: String = String::new(),
    }
}

section! {
    /// A directory as the node sees it and as the Docker daemon sees it.
    NodeDockerPathMap {
        /// Directory visible to the node.
        node: String = String::new(),
        /// The same directory on the daemon's host.
        host: String = String::new(),
    }
}

section! {
    /// `[node.docker]`
    NodeDocker {
        /// Exact image references admitted by the machine policy; * permits any.
        allowed_images:Vec<String> = list(&["*"]),
        /// Registry authorities admitted by the machine policy.
        allowed_registries:Vec<String> = list(&["*"]),
        /// Native and Linux capability keys admitted by the machine policy.
        allowed_capabilities:Vec<String> = list(&["*"]),
        /// Permitted runtime classes, within those the daemon reports.
        runtime_classes:Vec<String> = list(&["*"]),
        /// Privileged containers require an explicit operator setting, including in Full Access.
        allow_privileged:bool = false,
        /// Whether image archives may be loaded; narrowed image/registry rules refuse unverified archives.
        allow_archives:bool = true,
        /// Trusted image for restricted proxy sidecars and host-address observation; unset refuses restriction.
        restricted_proxy_image: Option<String> = None,
        /// Trusted preloaded image for the node's one-shot live mount/device helper.
        live_helper_image: Option<String> = None,
        /// Static orch-node binary; when unset the running node executable is used if it is static.
        live_helper_binary: Option<std::path::PathBuf> = None,
        /// Preloaded image for native build checks; empty uses the release's environment base image.
        conformance_image: Option<String> = None,
        /// The Docker daemon's socket.
        socket: String = s("/var/run/docker.sock"),
        /// What `gateway.orch.internal` resolves to inside its containers.
        gateway_host: String = s("host-gateway"),
        /// Whether this host builds images from Dockerfiles.
        builds: bool = true,
        /// Nonsecret defaults for Dockerfile proxy and repository arguments; template arguments take precedence.
        build_args: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new(),
        /// Carried-in image references mapped to verified local content IDs; no registry lookup for these images.
        offline_images: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new(),
        /// The longest one build may take.
        build_timeout_secs: u64 = 1_800,
        /// Builds running at once: one, because a build takes the processors agents are using.
        build_parallel: u32 = 1,
        /// Accepted from older files; cache removal follows the server's cleanup setting.
        build_cache_gb: u64 = 20,
        /// Private registry logins, with credentials kept in separate protected files.
        registries: Vec<NodeDockerRegistry> = Vec::new(),
        /// Paths whose names differ between the node and the daemon.
        path_map: Vec<NodeDockerPathMap> = Vec::new(),
        /// Whether the node can verify daemon bind sources in its own file system.
        check_host_paths: bool = true,
        /// Repository for images the driver builds or loads.
        image_repository: String = s("orch-build"),
        /// Additional labels on the driver's own images, networks and volumes.
        labels: BTreeMap<String, String> = BTreeMap::new(),
        /// The addresses every unit's own network is cut from.
        networks: NodeDockerNetworks = NodeDockerNetworks::default(),
        /// What the operator declares of each runtime the daemon has, by the name the daemon knows it by: what holds a
        /// unit in, which the daemon cannot tell. A runtime not named here counts as a shared kernel.
        runtimes: BTreeMap<String, NodeDockerRuntime> = BTreeMap::new(),
        /// How far the file may grow that Docker inside keeps its data in, in GB; the file is sparse.
        docker_inside_gb: u64 = 100,
        /// Name servers of a unit under a runtime with a kernel of its own, whose daemon's resolver cannot be reached
        /// from inside; empty reads the host's own.
        resolvers: Vec<String> = Vec::new(),
    }
}

section! {
    /// `[node.docker.runtimes.<name>]`: what is declared of one runtime of the daemon.
    NodeDockerRuntime {
        /// What holds a unit in: `none`, `shared_kernel`, `user_space_kernel` (gVisor) or `own_kernel` (a virtual
        /// machine, such as Kata). The boundary decides which capabilities are offered under the runtime and how
        /// risky they are, so it is the operator's word and never guessed from the name.
        boundary: String = s("shared_kernel"),
        /// How the runtime is called on screens, where its own name will not do; empty takes the usual name.
        title: String = String::new(),
        /// Whether software installed outside the volumes survives a stop under it. Unset takes what the runtime
        /// usually does; set it for gVisor started with `--overlay2=none`, which then keeps it as runc does.
        keeps_software_on_stop: Option<bool> = None,
        /// What each unit costs the host beyond its own limits, in MiB, counted when a host is judged full; unset
        /// takes the usual figure for the boundary.
        memory_overhead_mb: Option<i64> = None,
        /// Seconds a new unit's connector has to enrol under this runtime; unset takes the usual figure for the
        /// boundary, or the node's own when the boundary adds nothing.
        enrol_timeout_secs: Option<u64> = None,
    }
}

section! {
    /// `[node.docker.networks]`: every unit, open or restricted, is put on a network of its own cut from this
    /// pool, so that the daemon's own address pools and configuration stay as they are.
    NodeDockerNetworks {
        /// An IPv4 range of the private space that the daemon's default pools (172.17.0.0/16 and up,
        /// 192.168.0.0/16) do not hand out.
        pool: String = s("10.199.0.0/16"),
        /// The prefix length of one unit's network: a /28 has sixteen addresses, of which a unit and its proxy need
        /// two, and a /16 pool holds 4,096 of them. At most 29.
        prefix: u8 = 28,
    }
}

/// One named machine storage, shared with the enforcing machine policy.
pub type NodeStorage = orch_link_contract::policy::MachineStorage;

/// Previous-release `[node.hooks]`, imported once into the machine's local hooks policy.
pub type NodeHooks = orch_link_contract::policy::MachineHooks;

section! {
    /// `[node]`: the environment host.
    Node {
        /// Where it keeps its own state.
        data_dir: String = s("/var/lib/orch-node"),
        /// The gateway it connects to.
        gateway_url: String = String::new(),
        /// The built-in drivers it loads.
        drivers: Vec<String> = list(&["docker"]),
        /// How long a new runtime's connector has to enroll before its creation fails.
        enroll_timeout_secs: u64 = 120,
        /// How often it compares its units with the system's rows.
        reconcile_secs: u64 = 30,
        /// Time between asking a unit to stop and killing it.
        stop_grace_secs: u64 = 10,
        /// How often it looks up the digest behind each image tag in use.
        image_check_hours: u64 = 0,
        /// The Docker driver's own settings.
        docker: NodeDocker = NodeDocker::default(),
        /// Host-driver records and default volumes; choose a traversable base when using another account.
        host: NodeHost = NodeHost::default(),
        /// Named places for volumes; none means one, where the driver keeps its own.
        storage: Vec<NodeStorage> = Vec::new(),
        /// Legacy local lifecycle programs; accepted on upgrade and imported into `policy.hooks`.
        hooks: NodeHooks = NodeHooks::default(),
    }
}

section! {
    /// `[node.host]`: process-driver files, separate from the node's private link identity.
    NodeHost {
        /// Template identifiers this process driver may run.
        allowed_templates:Vec<String> = list(&["*"]),
        /// Existing local accounts permitted by the machine policy.
        allowed_accounts:Vec<String> = list(&["*"]),
        /// Absolute template launch programs, or *; agent execution remains bounded by its account/sandbox.
        allowed_programs:Vec<String> = list(&["*"]),
        /// Absolute base for records and default volumes; absent uses data_dir/host for the node's own account.
        base: Option<PathBuf> = None,
        /// Probe native systemd scopes, otherwise report resource use without enforcement.
        scopes: bool = true,
    }
}

section! {
    /// `[ports]`: the ceiling a host's operator sets for the host ports its node may publish. An
    /// absent member sets no ceiling; `ranges = []` permits no host opening at all. Only the node
    /// reads it, and no page of the host can widen it.
    Ports {
        /// The address callers are given, fixed: a name or an address.
        incoming_address: Option<String> = None,
        /// `all` (every address of both families, or any narrower one), or the one local IP permitted.
        listen: Option<String> = None,
        /// The ranges a declaration's ranges must lie in.
        ranges: Option<Vec<PortsRange>> = None,
    }
}

section! {
    /// One inclusive range of port numbers.
    PortsRange {
        /// Its first number.
        from: u16 = 0,
        /// Its last number.
        to: u16 = 0,
    }
}

section! {
    /// `[[plugin]]`: an external implementation of an extension point.
    Plugin {
        /// `provider`, `driver`, `adapter`, `executor`, `translator`, `summarizer` or `notifier`.
        kind: String = String::new(),
        /// Its name.
        name: String = String::new(),
        /// The program and its arguments.
        command: Vec<String> = Vec::new(),
        /// An already running local program: `unix:<path>` or `tcp://127.0.0.1:<port>`; exactly one of this and command.
        connect: String = String::new(),
        /// Its environment.
        env: BTreeMap<String, String> = BTreeMap::new(),
        /// When it is started again: `on-failure`, `always` or `never`.
        restart: String = s("on-failure"),
    }
}

section! {
    /// `[engine]`
    Engine {
        /// How long a plan for a fork, a switch, a move or an update may be executed.
        preflight_ttl_secs: u64 = 900,
        /// How long a job waits for an interrupted turn to end.
        interrupt_wait_secs: u64 = 60,
        /// After how long an unanswered interrupt is forced.
        interrupt_force_secs: u64 = 30,
        /// The longest that what a person typed for a sign-in is kept while its runtime is made.
        login_pending_secs: u64 = 4_500,
        /// The shortest time between two checks of a running turn against its caps.
        cap_check_secs: u64 = 5,
        /// How long the text an item gains while it grows is held before it is written to the item; every piece
        /// reaches live readers at once, and anything else the session says writes it first. 0 writes each piece.
        stream_flush_ms: u64 = 1_000,
        /// A provider page reads at most once per minute by default.
        provider_facts_page_secs: u64 = 60,
        /// A provider with recent bound-thread work reads every five minutes.
        provider_facts_active_secs: u64 = 300,
        /// Fifteen minutes retain a short working pause on the active cadence.
        provider_facts_active_recent_secs: u64 = 900,
        /// Idle providers read hourly, twenty-four times a day.
        provider_facts_idle_secs: u64 = 3600,
        /// Refused online providers find externally repaired credentials within five minutes.
        provider_facts_refused_secs: u64 = 300,
        /// How often the engine looks for what is due for removal.
        reaper_secs: u64 = 30,
        /// The longest pause between two attempts to remove something that will not go.
        reaper_retry_max_secs: u64 = 3_600,
        /// How long before an unused environment is removed its owner is told.
        removal_notice_hours: u64 = 24,
        /// The transfer rate assumed before a machine was measured.
        assumed_rate_bytes_per_sec: u64 = 4_194_304,
        /// A job that ran longer than this tells its creator when it ends.
        job_notify_after_secs: u64 = 30,
        /// How often an environment's resources are sampled.
        stats_interval_secs: u64 = 5,
        /// How much of those samples is kept in memory.
        stats_history_hours: u64 = 6,
    }
}

section! {
    /// `[retention]`
    Retention {
        /// Deleted things are purged after this.
        soft_delete_days: u64 = 30,
        /// An unreferenced blob is kept this long.
        blob_grace_hours: u64 = 24,
        /// How long the hourly history of measurements is kept; 0 keeps it for good.
        metrics_days: u64 = 0,
        /// How long each minute is kept before only its hour is; 0 keeps every minute.
        metrics_fine_days: u64 = 30,
    }
}

section! {
    /// `[metrics]`
    Metrics {
        /// Whether the server stores measurements at all.
        store: bool = true,
    }
}

section! {
    /// `[limits]`
    Limits {
        /// Recent visits kept for each person.
        recents_per_user: u32 = 50,
        /// Text indexed for search, per item and per chat message.
        search_text_chars: u32 = 20_000,
        /// The largest text shown inline in the file viewer.
        preview_bytes: u64 = 2_097_152,
        /// Workspace listings kept in memory for machines that are offline.
        manifest_trees: u32 = 64,
        /// How many orgs a path may hold one inside another, the root counted.
        org_depth: u32 = 8,
        /// How many projects a path may hold one inside another: a project in an org is the first.
        project_depth: u32 = 8,
        /// How many exports of one person are written at the same time: two, so that one person's archives
        /// do not take the machine from everybody else's work.
        exports_per_user: u32 = 2,
        /// How many imports of one person run at the same time: two, as exports are, for the same reason.
        imports_per_user: u32 = 2,
        /// The largest single file an export takes in: 256 MiB. A larger one is left out and listed.
        export_file_max_bytes: u64 = 268_435_456,
    }
}

section! {
    /// `[documents]`
    Documents {
        /// A version is cut before a change when the last was cut longer ago than this.
        version_window_secs: u64 = 600,
        /// A version is cut when a changed document has been left alone this long.
        version_idle_secs: u64 = 120,
    }
}

section! {
    /// `[realtime]`
    Realtime {
        /// How much may wait for one browser before the server stops counting on it.
        buffer_max_bytes: u64 = 4_194_304,
        /// For how long that much has to have waited before the connection is closed.
        buffer_max_secs: u64 = 30,
    }
}

section! {
    /// `[log]`
    Log {
        /// `text` or `json`.
        format: String = s("text"),
    }
}

/// Whether the ports domain and the system's own host are so related that a program served under the
/// one could set a cookie the other is sent: the same name, or one under the other. A domain that
/// shares only a parent with the system's host is allowed; a domain of its own is advised.
fn shares_origin(ports_domain: &str, main_host: &str) -> bool {
    !main_host.is_empty()
        && (ports_domain == main_host
            || main_host.ends_with(&format!(".{ports_domain}"))
            || ports_domain.ends_with(&format!(".{main_host}")))
}

impl Validate for ServerConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        let invalid = |key: &str, message: &str| ConfigError::Invalid {
            key: key.to_owned(),
            message: message.to_owned(),
        };
        if self.version != 1 {
            return Err(invalid("version", "this build reads version 1"));
        }
        if self.release.request_timeout_secs == 0 || self.release.manifest_max_bytes == 0 {
            return Err(invalid(
                "release",
                "the manifest's time and size bounds must be positive",
            ));
        }
        if !self.release.url.is_empty() {
            let url = self
                .release
                .url
                .parse::<http::Uri>()
                .map_err(|_| invalid("release.url", "must be an HTTP(S) base address"))?;
            if !matches!(url.scheme_str(), Some("http" | "https"))
                || url.host().is_none()
                || url
                    .authority()
                    .is_some_and(|authority| authority.as_str().contains('@'))
                || url.query().is_some()
                || self.release.url.contains('#')
            {
                return Err(invalid(
                    "release.url",
                    "must be an HTTP(S) base address without credentials, query or fragment",
                ));
            }
        }
        if let Some(domain) = &self.edge.ports_domain {
            let domain = domain.trim().trim_end_matches('.').to_ascii_lowercase();
            let main = self
                .system
                .public_url
                .parse::<http::Uri>()
                .ok()
                .and_then(|uri| uri.host().map(str::to_ascii_lowercase))
                .unwrap_or_default();
            if !domain.is_empty() && shares_origin(&domain, &main) {
                return Err(invalid(
                    "edge.ports_domain",
                    "must not be the system's own host, a domain above it or one below it: a program served under it could set cookies that the system's pages are sent",
                ));
            }
        }
        if let Some(listen) = &self.ports.listen
            && listen != "all"
            && listen.parse::<std::net::IpAddr>().is_err()
        {
            return Err(invalid("ports.listen", "is all, or one literal IP address"));
        }
        if self
            .ports
            .ranges
            .iter()
            .flatten()
            .any(|range| range.from == 0 || range.from > range.to)
        {
            return Err(invalid(
                "ports.ranges",
                "each range runs from a number of 1 or more to one no smaller",
            ));
        }
        for cidr in &self.api.trusted_proxy_cidrs {
            if cidr.parse::<ipnet::IpNet>().is_err() {
                return Err(invalid(
                    "api.trusted_proxy_cidrs",
                    "must contain IP network ranges in CIDR notation",
                ));
            }
        }
        let networks = &self.node.docker.networks;
        match networks.pool.parse::<ipnet::Ipv4Net>() {
            Ok(pool) if networks.prefix >= pool.prefix_len() && networks.prefix <= 29 => {}
            Ok(_) => {
                return Err(invalid(
                    "node.docker.networks.prefix",
                    "is at least the pool's own prefix length and at most 29, so that the pool holds one unit's network and a network holds a unit and its proxy",
                ));
            }
            Err(_) => {
                return Err(invalid(
                    "node.docker.networks.pool",
                    "is an IPv4 network range in CIDR notation",
                ));
            }
        }
        if self.link.dead_after_secs <= self.link.heartbeat_secs {
            return Err(invalid(
                "link.dead_after_secs",
                "must be longer than link.heartbeat_secs",
            ));
        }
        if !matches!(self.log.format.as_str(), "text" | "json") {
            return Err(invalid("log.format", "must be text or json"));
        }
        for endpoint in &self.gateway.endpoints {
            if !matches!(endpoint.trust.as_str(), "pinned" | "public") {
                return Err(invalid(
                    "gateway.endpoints.trust",
                    "must be pinned or public",
                ));
            }
        }
        for plugin in &self.plugin {
            if !orch_plugin_contract::registration::PluginKind::ALL
                .iter()
                .any(|kind| kind.as_str() == plugin.kind)
            {
                return Err(invalid("plugin.kind", "is not a kind of extension"));
            }
            plugin.registration()?;
        }
        if self.auth.login_failure_limit == 0 || self.auth.address_failure_limit == 0 {
            return Err(invalid("auth", "a limit of wrong tries cannot be 0"));
        }
        if self.limits.org_depth == 0 {
            return Err(invalid("limits.org_depth", "must be at least 1"));
        }
        if self.limits.project_depth == 0 {
            return Err(invalid("limits.project_depth", "must be at least 1"));
        }
        if self.limits.exports_per_user == 0 {
            return Err(invalid("limits.exports_per_user", "must be at least 1"));
        }
        if self.limits.imports_per_user == 0 {
            return Err(invalid("limits.imports_per_user", "must be at least 1"));
        }
        Ok(())
    }
}

impl Plugin {
    /// Reads this entry as the shared registration, validating its local address and start form before a host uses it.
    pub fn registration(
        &self,
    ) -> Result<orch_plugin_contract::registration::PluginRegistration, ConfigError> {
        use orch_plugin_contract::registration::{PluginKind, PluginRegistration, Restart};
        let invalid = |message: String| ConfigError::Invalid {
            key: "plugin".into(),
            message,
        };
        let kind = PluginKind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == self.kind)
            .ok_or_else(|| invalid("is not a kind of extension".into()))?;
        let restart = match self.restart.as_str() {
            "" => None,
            "never" => Some(Restart::Never),
            "on-failure" => Some(Restart::OnFailure),
            "always" => Some(Restart::Always),
            _ => {
                return Err(invalid(
                    "restart must be never, on-failure or always".into(),
                ));
            }
        };
        let registration = PluginRegistration {
            kind,
            name: self.name.clone(),
            command: (!self.command.is_empty()).then(|| self.command.clone()),
            connect: (!self.connect.is_empty()).then(|| self.connect.clone()),
            env: self.env.clone(),
            restart,
        };
        registration
            .start()
            .map_err(|error| invalid(error.to_string()))?;
        Ok(registration)
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use orch_plugin_contract::registration::PluginStart;

    #[test]
    fn every_contract_kind_is_read_from_config_and_reaches_the_host_that_owns_it() {
        use orch_plugin_contract::registration::{PluginHost, PluginKind};
        for kind in PluginKind::ALL {
            let text = format!(
                "[[plugin]]\nkind={:?}\nname='external'\ncommand=['program']\n",
                kind.as_str()
            );
            let config: ServerConfig =
                crate::loader::load_text(&text, "orch.toml", |_| None).unwrap();
            let registration = config.plugin[0].registration().unwrap();
            assert_eq!(registration.kind, kind);
            assert!(
                matches!(registration.start().unwrap(),PluginStart::Command {argv,..} if argv == ["program"])
            );
            match kind {
                PluginKind::Provider
                | PluginKind::Translator
                | PluginKind::Adapter
                | PluginKind::Executor => {
                    assert_eq!(registration.kind.host(), PluginHost::Connector)
                }
                PluginKind::Driver => assert_eq!(registration.kind.host(), PluginHost::Node),
                PluginKind::Summarizer => assert_eq!(registration.kind.host(), PluginHost::Api),
                PluginKind::Notifier => assert_eq!(registration.kind.host(), PluginHost::Worker),
            }
        }
        assert!(
            crate::loader::load_text::<ServerConfig>(
                "[[plugin]]\nkind='unknown'\nname='external'\ncommand=['program']\n",
                "orch.toml",
                |_| None
            )
            .is_err()
        );
    }

    #[test]
    fn a_ports_domain_that_shares_the_systems_own_origin_is_refused() {
        let with = |domain: &str| {
            let mut config = ServerConfig::default();
            config.system.public_url = "https://orch.example.com".to_owned();
            config.edge.ports_domain = Some(domain.to_owned());
            config.validate()
        };
        assert!(with("ports.example.net").is_ok());
        assert!(
            with("ports.example.com").is_ok(),
            "a sibling shares only a parent"
        );
        assert!(with("").is_ok(), "an empty domain is no domain");
        for shared in [
            "orch.example.com",
            "example.com",
            "p.orch.example.com",
            "ORCH.example.com.",
        ] {
            assert!(with(shared).is_err(), "{shared}");
        }
    }

    #[test]
    fn an_operator_can_attach_a_local_service_without_naming_a_program_to_start() {
        let config: ServerConfig = toml::from_str(
            "[[plugin]]\nkind='driver'\nname='external'\nconnect='tcp://127.0.0.1:41880'\n",
        )
        .unwrap();
        assert!(config.validate().is_ok());
        assert!(matches!(
            config.plugin[0].registration().unwrap().start(),
            Ok(PluginStart::Attach(_))
        ));
        assert!(config.plugin[0].command.is_empty());
    }

    #[test]
    fn an_attach_entry_refuses_both_forms_and_an_address_outside_the_local_machine() {
        for members in [
            "command=['program']\nconnect='tcp://127.0.0.1:41880'",
            "connect='tcp://192.0.2.1:41880'",
            "connect='tcp://127.0.0.1:0'",
            "",
        ] {
            let config: ServerConfig = toml::from_str(&format!(
                "[[plugin]]\nkind='driver'\nname='external'\n{members}\n"
            ))
            .unwrap();
            assert!(config.validate().is_err(), "{members}");
        }
    }

    #[test]
    fn a_nodes_unit_networks_are_read_from_its_own_table_and_checked() {
        let config: ServerConfig =
            toml::from_str("[node.docker.networks]\npool='10.77.0.0/20'\nprefix=27\n").unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.node.docker.networks.prefix, 27);
        let shipped = ServerConfig::default();
        assert_eq!(shipped.node.docker.networks.pool, "10.199.0.0/16");
        for wrong in [
            "[node.docker.networks]\npool='fd00::/64'\n",
            "[node.docker.networks]\npool='10.77.0.0/20'\nprefix=30\n",
            "[node.docker.networks]\npool='10.77.0.0/20'\nprefix=16\n",
        ] {
            let config: ServerConfig = toml::from_str(wrong).unwrap();
            assert!(config.validate().is_err(), "{wrong}");
        }
        assert!(
            toml::from_str::<ServerConfig>("[node.hooks]\npre_start='/bin/true'\n").is_err(),
            "a misspelled hook is refused, never ignored"
        );
    }
}
