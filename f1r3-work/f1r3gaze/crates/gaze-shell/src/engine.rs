//! What every tab of a profile shares, and the per-tab services.

use crate::pages;
// Was `use crate::profile::{Settings, user_id};`: the engine opened the
// single profile folder itself (`Engine::new`).
use crate::profile::reconcile::Managed;
use crate::profile::{Profile, Settings};
use crate::site_index::{index_bytes, read_index, salvage_index};
use gaze_blob::{Blobs, ContentCache};
use crate::grants::FileGrants;
use gaze_fs::Perm;
use gaze_broker::{Broker, NAV, NET, Refusal, SHARD, STORE, ShardClass, Site};
use gaze_dom_blitz::{Delivery, Services, WakeHandle};
use gaze_exec::{CapRequest, Class};
use gaze_net::{Http, NetError, Pool, Schemes, content_hash, fetch_reply, fetch_request, unhex};
use gaze_shard::{
    Bridge, DriveSource, FileFreshness, FileKeystore, FreshnessLog, Keystore, MemFreshness, Payer, ShardOut,
    ShardService, SiteAddr, SiteManifest,
};
use gaze_store::{OriginStore, path_for};
use gaze_wallet::{Embers, Limits, Wallets};
use k1ndl1ng_norm::{Name, Node, Norm};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Engine {
    // Was the single profile folder, which held everything (`settings.conf`,
    // `cache/`, `keys/`, `grants.tsv`, `store/`, …). The files are in the
    // profile's five roots now, at `profile.layout`'s places.
    // pub dir: PathBuf,
    /// The open profile: its roots, what start-up found, and the lock.
    pub profile: Profile,
    /// The settings as start-up loaded them. The theme chosen since is
    /// `profile.theme()`.
    pub settings: Settings,
    pub http: Http,
    pub pool: Pool,
    pub schemes: Schemes,
    pub blobs: Arc<Blobs>,
    pub bridge: Arc<Bridge>,
    pub wallets: Arc<Wallets>,
    pub broker: RefCell<Broker<FileGrants>>,
    pub sites: Arc<SiteCache>,
    stores: RefCell<BTreeMap<Site, Rc<RefCell<OriginStore>>>>,
    store_index: RefCell<Vec<String>>,
}

/// Resolved site manifests, briefly cached (sub-resources of one page load
/// should not each resolve the site).
pub struct SiteCache {
    bridge: Arc<Bridge>,
    map: Mutex<BTreeMap<String, (Instant, SiteManifest)>>,
}

impl SiteCache {
    pub fn manifest(&self, a: &SiteAddr) -> Result<SiteManifest, String> {
        let key = a.registry_uri();
        if let Ok(m) = self.map.lock()
            && let Some((t, m)) = m.get(&key)
            && t.elapsed() < Duration::from_secs(15)
        {
            return Ok(m.clone());
        }
        let (_, m) = self.bridge.resolve_site(a)?;
        if let Ok(mut map) = self.map.lock() {
            map.insert(key, (Instant::now(), m.clone()));
        }
        Ok(m)
    }
    pub fn file(&self, url: &str) -> Result<(String, Vec<u8>), String> {
        let a = SiteAddr::parse(url).ok_or_else(|| format!("bad f1r3 address {url}"))?;
        let m = self.manifest(&a)?;
        self.bridge.site_file(&m, &a.path)
    }
}

impl Engine {
    /// The engine of an open profile: the content cache, the keystore and
    /// wallets, the freshness records, the permission decisions and the site
    /// stores, each at its place in the profile's roots. What the session
    /// may not write is opened read-only: everything when the session only
    /// reads, or a file start-up could not read.
    pub fn open(profile: Profile) -> Rc<Engine> {
        let settings = profile.settings.clone();
        let layout = profile.layout.clone();
        let http = Http::new();
        let pool = Pool::new(6);
        let cache = ContentCache::new(layout.content_dir(), settings.cache_bytes);
        let cache = match profile.read_only_reason() {
            Some(why) => cache.read_only(why),
            None => cache,
        };
        let blobs = Arc::new(Blobs::new(cache, http.clone()));
        for m in &settings.mirrors {
            blobs.add_source(Arc::new(gaze_blob::MirrorSource::new(m, http.clone())));
        }
        let mut shard_cfg = settings.shard.clone();
        // The id start-up read, repaired or made (never a new one here).
        shard_cfg.user = profile.user_id().to_string();
        // The keystore start-up checked: the credential store on macOS and
        // Windows in `os-keyring` builds, files everywhere else.
        #[cfg(feature = "os-keyring")]
        let keys: Arc<dyn Keystore> = match profile.keystore() {
            crate::profile::KeystoreKind::Os => Arc::new(gaze_shard::keys::OsKeystore::new("F1R3Gaze")),
            crate::profile::KeystoreKind::File => Arc::new(FileKeystore::new(layout.keys_dir())),
        };
        #[cfg(not(feature = "os-keyring"))]
        let keys: Arc<dyn Keystore> = Arc::new(FileKeystore::new(layout.keys_dir()));
        // The agent driving the browser pays: every deploy is signed by the
        // active wallet, whose keys live in the keystore.
        let embers = settings.embers_api.as_ref().map(|base| {
            Embers::new(
                base,
                http.clone(),
                Limits {
                    shard_id: settings.shard.shard_id.clone(),
                    max_fee: settings.max_fee,
                },
            )
        });
        let wallets = Wallets::open(layout.wallet_dir(), keys, embers);
        let wallets = Arc::new(match profile.blocked.wallets() {
            Some(why) => wallets.read_only(why),
            None => wallets,
        });
        let payer: Arc<dyn Payer> = wallets.clone();
        let bridge = Bridge::new(shard_cfg, http.clone(), pool.clone(), payer, Arc::clone(&blobs), freshness_log(&profile));
        blobs.add_source(Arc::new(DriveSource {
            bridge: Arc::clone(&bridge),
            root: "/gaze-blob/".into(),
        }));
        let sites = Arc::new(SiteCache {
            bridge: Arc::clone(&bridge),
            map: Mutex::new(BTreeMap::new()),
        });
        let schemes = Schemes::default();
        {
            let b = Arc::clone(&blobs);
            schemes.register(
                "f1r3h",
                Arc::new(move |u: &str| {
                    let h = content_hash(u).ok_or_else(|| NetError::Url(u.into()))?;
                    b.get(&h, &[]).map_err(NetError::NotFound)
                }),
            );
            let s = Arc::clone(&sites);
            schemes.register(
                "f1r3",
                Arc::new(move |u: &str| s.file(u).map(|(_, b)| b).map_err(NetError::NotFound)),
            );
            schemes.register(
                "gaze",
                Arc::new(|u: &str| {
                    pages::builtin(u)
                        .map(|s| s.into_bytes())
                        .ok_or_else(|| NetError::NotFound(u.into()))
                }),
            );
            schemes.register(
                "file",
                Arc::new(|u: &str| {
                    let p = url::Url::parse(u)
                        .ok()
                        .and_then(|x| x.to_file_path().ok())
                        .ok_or_else(|| NetError::Url(u.into()))?;
                    std::fs::read(p).map_err(|e| NetError::NotFound(e.to_string()))
                }),
            );
        }
        let grants = FileGrants::new(layout.grants_file());
        let broker = RefCell::new(Broker::new(match profile.blocked.why(Managed::Grants) {
            Some(why) => grants.read_only(why),
            None => grants,
        }));
        // Start-up repaired a damaged index; one it could only read (a
        // session that only reads) keeps what can be read of it.
        let store_index = match profile.fs().read(&layout.origins_file()) {
            Ok(bytes) => read_index(&bytes).unwrap_or_else(|_| salvage_index(&bytes).0),
            Err(_) => Vec::new(),
        };
        Rc::new(Engine {
            profile,
            settings,
            http,
            pool,
            schemes,
            blobs,
            bridge,
            wallets,
            broker,
            sites,
            stores: RefCell::new(BTreeMap::new()),
            store_index: RefCell::new(store_index),
        })
    }

    // Disabled at the switch-over (ledger S14): it opened the single profile
    // folder, with no instance lock, no start-up and no repair. Every caller
    // opens a `Profile` (start-up's order) and then `Engine::open`.
    // pub fn new(dir: PathBuf) -> Rc<Engine> {
    //     let settings = Settings::load(&dir);
    //     let http = Http::new();
    //     let pool = Pool::new(6);
    //     let blobs = Arc::new(Blobs::new(
    //         ContentCache::new(dir.join("cache"), settings.cache_bytes),
    //         http.clone(),
    //     ));
    //     for m in &settings.mirrors {
    //         blobs.add_source(Arc::new(gaze_blob::MirrorSource::new(m, http.clone())));
    //     }
    //     let mut shard_cfg = settings.shard.clone();
    //     shard_cfg.user = user_id(&dir);
    //     #[cfg(feature = "os-keyring")]
    //     let keys: Arc<dyn Keystore> = if cfg!(any(target_os = "macos", windows)) {
    //         Arc::new(gaze_shard::keys::OsKeystore::new("F1R3Gaze"))
    //     } else {
    //         Arc::new(FileKeystore::new(dir.join("keys")))
    //     };
    //     #[cfg(not(feature = "os-keyring"))]
    //     let keys: Arc<dyn Keystore> = Arc::new(FileKeystore::new(dir.join("keys")));
    //     // The agent driving the browser pays: every deploy is signed by the
    //     // active wallet, whose keys live in the keystore.
    //     let embers = settings.embers_api.as_ref().map(|base| {
    //         Embers::new(
    //             base,
    //             http.clone(),
    //             Limits {
    //                 shard_id: settings.shard.shard_id.clone(),
    //                 max_fee: settings.max_fee,
    //             },
    //         )
    //     });
    //     let wallets = Arc::new(Wallets::open(dir.clone(), keys, embers));
    //     let payer: Arc<dyn Payer> = wallets.clone();
    //     let bridge = Bridge::new(
    //         shard_cfg,
    //         http.clone(),
    //         pool.clone(),
    //         payer,
    //         Arc::clone(&blobs),
    //         Arc::new(gaze_shard::MemFreshness::default()),
    //     );
    //     blobs.add_source(Arc::new(DriveSource {
    //         bridge: Arc::clone(&bridge),
    //         root: "/gaze-blob/".into(),
    //     }));
    //     let sites = Arc::new(SiteCache {
    //         bridge: Arc::clone(&bridge),
    //         map: Mutex::new(BTreeMap::new()),
    //     });
    //     let schemes = Schemes::default();
    //     {
    //         let b = Arc::clone(&blobs);
    //         schemes.register(
    //             "f1r3h",
    //             Arc::new(move |u: &str| {
    //                 let h = content_hash(u).ok_or_else(|| NetError::Url(u.into()))?;
    //                 b.get(&h, &[]).map_err(NetError::NotFound)
    //             }),
    //         );
    //         let s = Arc::clone(&sites);
    //         schemes.register(
    //             "f1r3",
    //             Arc::new(move |u: &str| s.file(u).map(|(_, b)| b).map_err(NetError::NotFound)),
    //         );
    //         schemes.register(
    //             "gaze",
    //             Arc::new(|u: &str| {
    //                 pages::builtin(u)
    //                     .map(|s| s.into_bytes())
    //                     .ok_or_else(|| NetError::NotFound(u.into()))
    //             }),
    //         );
    //         schemes.register(
    //             "file",
    //             Arc::new(|u: &str| {
    //                 let p = url::Url::parse(u)
    //                     .ok()
    //                     .and_then(|x| x.to_file_path().ok())
    //                     .ok_or_else(|| NetError::Url(u.into()))?;
    //                 std::fs::read(p).map_err(|e| NetError::NotFound(e.to_string()))
    //             }),
    //         );
    //     }
    //     let broker = RefCell::new(Broker::new(FileGrants::new(dir.join("grants.tsv"))));
    //     let store_index = std::fs::read(dir.join("store/origins.json"))
    //         .ok()
    //         .and_then(|b| serde_json::from_slice(&b).ok())
    //         .unwrap_or_default();
    //     Rc::new(Engine {
    //         dir,
    //         settings,
    //         http,
    //         pool,
    //         schemes,
    //         blobs,
    //         bridge,
    //         wallets,
    //         broker,
    //         sites,
    //         stores: RefCell::new(BTreeMap::new()),
    //         store_index: RefCell::new(store_index),
    //     })
    // }

    pub fn store_sites(&self) -> Vec<String> {
        self.store_index.borrow().clone()
    }

    /// Saves the list of sites with a store, unless the session must not
    /// write it (read-only, or a file start-up could not read: it is never
    /// replaced).
    fn save_store_index(&self) {
        if self.profile.blocked.why(Managed::SiteIndex).is_some() {
            return;
        }
        let (fs, layout) = (self.profile.fs(), &self.profile.layout);
        let bytes = index_bytes(&self.store_index.borrow());
        let _ = gaze_fs::create_dir_durably(fs, &layout.site_data_dir())
            .and_then(|_| gaze_fs::write_atomic(fs, &layout.origins_file(), &bytes, Perm::Private));
    }

    pub fn store_for(&self, site: &Site) -> Result<Rc<RefCell<OriginStore>>, String> {
        if let Some(s) = self.stores.borrow().get(site) {
            return Ok(Rc::clone(s));
        }
        // A store is an append-only log that its first write would change:
        // a session that only reads opens none.
        if let Some(why) = self.profile.read_only_reason() {
            return Err(format!("site data is not opened: {why}"));
        }
        let path = path_for(&self.profile.layout.site_data_dir(), site.as_str());
        // Was `&gaze_store::Discard`: a damaged store was cut back to its
        // readable records and the rest was lost. Now the whole file is kept
        // in this start's data backups first, and reported.
        let salvage = self.profile.store_salvage(site.as_str());
        let store = Rc::new(RefCell::new(
            OriginStore::open(path, self.settings.store_quota, &salvage).map_err(|e| format!("{e:?}"))?,
        ));
        self.stores
            .borrow_mut()
            .insert(site.clone(), Rc::clone(&store));
        if !self.store_index.borrow().iter().any(|s| s == site.as_str()) {
            self.store_index
                .borrow_mut()
                .push(site.as_str().to_string());
            self.save_store_index();
        }
        Ok(store)
    }

    pub fn clear_store(&self, site: &str) -> Result<(), String> {
        let Some(s) = Site::of_url(site) else {
            return Err("bad site".into());
        };
        if let Some(why) = self.profile.read_only_reason() {
            return Err(format!("site data is kept: {why}"));
        }
        self.stores.borrow_mut().remove(&s);
        // Was `std::fs::remove_file` when the file existed, with the removal
        // never synced: it could come back after a power cut.
        let (fs, dir) = (self.profile.fs(), self.profile.layout.site_data_dir());
        let p = path_for(&dir, s.as_str());
        match fs.remove_file(&p) {
            Ok(()) => fs.sync_dir(&dir).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        self.store_index.borrow_mut().retain(|x| x != site);
        self.save_store_index();
        Ok(())
    }

    /// Fetch a top-level document: `(final URL, HTML)`. Non-HTML content is
    /// wrapped in a minimal page. Runs on a worker thread.
    pub fn fetch_document(
        http: &Http,
        schemes: &Schemes,
        url: &str,
        https_only: bool,
    ) -> Result<(String, String), String> {
        if https_only && url.starts_with("http://") {
            return Err(format!("plain HTTP is disabled in settings: {url}"));
        }
        let (final_url, body, ctype) = if url.starts_with("http://") || url.starts_with("https://")
        {
            let r = http
                .send_following(
                    &gaze_net::HttpRequest {
                        url: url.into(),
                        method: "GET".into(),
                        headers: vec![(
                            "accept".into(),
                            "text/html, application/xhtml+xml, */*".into(),
                        )],
                        body: Vec::new(),
                    },
                    &|_| true,
                )
                .map_err(|e| e.to_string())?;
            if !(200..300).contains(&r.status) {
                return Err(format!("{url}: HTTP {}", r.status));
            }
            let ct = r.header("content-type").unwrap_or("text/html").to_string();
            (r.url, r.body, ct)
        } else {
            let (u, b) = gaze_net::fetch_url(http, schemes, url).map_err(|e| e.to_string())?;
            let ct = if u.ends_with(".txt") || u.ends_with(".rho") {
                "text/plain"
            } else {
                "text/html"
            };
            (u, b, ct.to_string())
        };
        let text = String::from_utf8_lossy(&body).into_owned();
        let html = if ctype.contains("html") {
            text
        } else if ctype.starts_with("image/") {
            format!(
                "<html><body style=\"margin:0\"><img src=\"{}\"></body></html>",
                escape(&final_url)
            )
        } else {
            format!("<html><body><pre>{}</pre></body></html>", escape(&text))
        };
        Ok((final_url, html))
    }
}

/// Where the bridge keeps its freshness records: in memory for a
/// development shard on this machine, which is reset often (storage plan,
/// decision 7); otherwise `data/trust/freshness.tsv`, enforced but never
/// written when this session must not write it.
pub fn freshness_log(profile: &Profile) -> Arc<dyn FreshnessLog> {
    let shard = &profile.settings.shard;
    match observers_are_loopback(&shard.observers) {
        true => Arc::new(MemFreshness::default()),
        false => {
            let records = FileFreshness::open(profile.layout.trust_file(), &shard.shard_id);
            Arc::new(match profile.blocked.why(Managed::Freshness) {
                Some(why) => records.read_only(why),
                None => records,
            })
        }
    }
}

/// Whether every observer runs on this machine (`localhost`, a name under
/// `.localhost`, 127.0.0.0/8 or `::1`): a development shard, which is reset
/// often, so its freshness records stay in memory (storage plan, decision
/// 7). An empty list counts too; an address that cannot be read does not.
pub fn observers_are_loopback(observers: &[String]) -> bool {
    observers.iter().all(|observer| match url::Url::parse(observer).ok().as_ref().and_then(url::Url::host) {
        Some(url::Host::Domain(name)) => {
            let name = name.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    })
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn as_name(n: &Norm) -> Option<Name> {
    match n.node() {
        Node::Eval(x) => Some(x.clone()),
        _ => None,
    }
}

fn err(code: &str, d: &str) -> Norm {
    Norm::tuple(vec![Norm::str("err"), Norm::str(code), Norm::str(d)])
}

/// A navigation a page or a link asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NavRequest {
    Go(String),
    Replace(String),
    Back,
    /// Cross-site: open in a new tab.
    Open(String),
}

enum Held {
    Net { site: Site, args: Vec<Norm> },
    Shard { class: ShardClass, args: Vec<Norm> },
}

/// One tab's live services. Shared between the page's `Services` and the
/// chrome, which answers prompts and drains navigations.
pub struct TabCore {
    pub tab: u64,
    pub site: Site,
    pub base_url: String,
    pub grant_hash: [u8; 32],
    eng: Rc<Engine>,
    pub shard: Option<ShardService>,
    async_out: Arc<Mutex<Vec<Delivery>>>,
    pub wake: WakeHandle,
    held: Vec<(u64, String, Held)>,
    next: u64,
    pub nav: Vec<NavRequest>,
}

impl TabCore {
    pub fn new(
        eng: Rc<Engine>,
        tab: u64,
        site: Site,
        base_url: &str,
        grant_hash: [u8; 32],
        wake: WakeHandle,
    ) -> TabCore {
        let w = wake.clone();
        let shard = Some(ShardService::new(
            Arc::clone(&eng.bridge),
            site.as_str(),
            Arc::new(move || w.wake()),
        ));
        TabCore {
            tab,
            site,
            base_url: base_url.to_string(),
            grant_hash,
            eng,
            shard,
            async_out: Arc::default(),
            wake,
            held: Vec::new(),
            next: 0,
            nav: Vec::new(),
        }
    }

    fn resolve(&self, u: &str) -> Option<String> {
        url::Url::parse(&self.base_url)
            .ok()?
            .join(u)
            .ok()
            .map(|x| x.to_string())
            .or_else(|| url::Url::parse(u).ok().map(|x| x.to_string()))
    }

    fn hold(&mut self, text: String, h: Held) {
        self.next += 1;
        self.held.push((self.next, text, h));
        self.wake.wake();
    }

    /// Prompts for the chrome: `(id, text)`. Ids at or above `1 << 40` are
    /// the shard service's own (deploy and session consent).
    pub fn prompts(&self) -> Vec<(u64, String)> {
        let mut v: Vec<(u64, String)> = self.held.iter().map(|(i, t, _)| (*i, t.clone())).collect();
        if let Some(s) = &self.shard {
            v.extend(s.prompts().into_iter().map(|p| ((1 << 40) + p.id, p.text)));
        }
        v
    }

    pub fn answer(&mut self, id: u64, yes: bool, remember: bool) {
        if id >= 1 << 40 {
            if let Some(s) = &mut self.shard {
                s.answer(id - (1 << 40), yes);
            }
            return;
        }
        let Some(i) = self.held.iter().position(|(x, _, _)| *x == id) else {
            return;
        };
        let (_, _, h) = self.held.remove(i);
        match h {
            Held::Net { site, args } => {
                if yes {
                    self.eng.broker.borrow_mut().allow_net(self.tab, site);
                    self.net(args);
                } else if let Some(r) = args.last().and_then(as_name) {
                    self.push(Class::Net, r, err("denied", "the user declined"));
                }
            }
            Held::Shard { class, args } => {
                if yes {
                    let gh = remember.then_some(self.grant_hash);
                    let _ = self
                        .eng
                        .broker
                        .borrow_mut()
                        .allow_shard(self.tab, class, gh);
                    if let Some(s) = &mut self.shard {
                        s.request(SHARD, &args);
                    }
                } else if let Some(r) = args.last().and_then(as_name) {
                    self.push(Class::Shard, r, err("denied", "the user declined"));
                }
            }
        }
    }

    fn push(&self, class: Class, chan: Name, datum: Norm) {
        if let Ok(mut o) = self.async_out.lock() {
            o.push(Delivery::reply(class, chan, datum));
        }
        self.wake.wake();
    }

    fn net(&mut self, args: Vec<Norm>) {
        let Some(ret) = args.last().and_then(as_name) else {
            return;
        };
        let f = match fetch_request(&args) {
            Ok(f) => f,
            Err(code) => return self.push(Class::Net, ret, err(code, "fetch")),
        };
        let url = match self.resolve(&f.request.url) {
            Some(u) => u,
            None => return self.push(Class::Net, ret, err("type", &f.request.url)),
        };
        let check = self.eng.broker.borrow().check_net(self.tab, &url);
        match check {
            Ok(()) => {
                let allowed = self
                    .eng
                    .broker
                    .borrow()
                    .route(self.tab, NET)
                    .map(|r| r.att.net_sites.clone())
                    .unwrap_or_default();
                let (http, schemes, out, wake) = (
                    self.eng.http.clone(),
                    self.eng.schemes.clone(),
                    Arc::clone(&self.async_out),
                    self.wake.clone(),
                );
                let mut f = f;
                f.request.url = url;
                self.eng.pool.spawn(move || {
                    let r = if f.request.url.starts_with("http") {
                        // Every redirect hop is checked against the allow-list.
                        http.send_following(&f.request, &|hop| {
                            Site::of_url(hop).is_some_and(|s| allowed.contains(&s))
                        })
                    } else {
                        gaze_net::fetch_url(&http, &schemes, &f.request.url).map(|(u, body)| {
                            gaze_net::HttpResponse {
                                url: u,
                                status: 200,
                                headers: Vec::new(),
                                body,
                            }
                        })
                    };
                    let datum = fetch_reply(&f, r);
                    if let Ok(mut o) = out.lock() {
                        o.push(Delivery::reply(Class::Net, ret, datum));
                    }
                    wake.wake();
                });
            }
            Err(Refusal::Ask(text)) => {
                let site = Site::of_url(&url).expect("checked");
                self.hold(text, Held::Net { site, args });
            }
            Err(Refusal::Denied(d)) => self.push(Class::Net, ret, err("denied", &d)),
            Err(Refusal::Revoked) => self.push(Class::Net, ret, err("revoked", "")),
        }
    }

    fn store(&mut self, args: &[Norm]) {
        let Some(ret) = args.last().and_then(as_name) else {
            // Writes without an acknowledgement still happen.
            if let Ok(s) = self.eng.store_for(&self.site) {
                let _ = s.borrow_mut().serve(args);
            }
            return;
        };
        if self.eng.broker.borrow().route(self.tab, STORE).is_none() {
            return self.push(Class::Store, ret, err("revoked", ""));
        }
        let datum = match self.eng.store_for(&self.site) {
            Ok(s) => s
                .borrow_mut()
                .serve(args)
                .unwrap_or_else(|| err("type", "store")),
            Err(_) => err("io", "store unavailable"),
        };
        self.push(Class::Store, ret, datum);
    }

    fn nav_req(&mut self, args: &[Norm]) {
        let verb = args.first().and_then(|v| v.as_str()).unwrap_or("");
        let target = args
            .get(1)
            .and_then(|v| v.as_str())
            .and_then(|u| self.resolve(u));
        let req = match (verb, target) {
            ("go", Some(u)) | ("replace", Some(u)) => {
                let same = Site::of_url(&u).is_some_and(|s| s.same_site(&self.site));
                if !same {
                    NavRequest::Open(u)
                } else if verb == "go" {
                    NavRequest::Go(u)
                } else {
                    NavRequest::Replace(u)
                }
            }
            ("back", _) => NavRequest::Back,
            _ => return,
        };
        self.nav.push(req);
        self.wake.wake();
    }

    fn shard_req(&mut self, urn: &str, args: Vec<Norm>) {
        if urn != SHARD {
            // A session name: consent was given when the session opened.
            if let Some(s) = &mut self.shard {
                s.request(urn, &args);
            }
            return;
        }
        let verb = args
            .first()
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let check = self.eng.broker.borrow().check_shard(self.tab, &verb);
        match check {
            Ok(_) => {
                if let Some(s) = &mut self.shard {
                    s.request(SHARD, &args);
                }
            }
            Err(Refusal::Ask(text)) => {
                let class = ShardClass::of_verb(&verb).unwrap_or(ShardClass::Read);
                self.hold(text, Held::Shard { class, args });
            }
            Err(e) => {
                if let Some(r) = args.last().and_then(as_name) {
                    let (c, d) = match e {
                        Refusal::Revoked => ("revoked", String::new()),
                        Refusal::Denied(d) => ("verb", d),
                        Refusal::Ask(d) => ("denied", d),
                    };
                    self.push(Class::Shard, r, err(c, &d));
                }
            }
        }
    }
}

/// The page's view of its tab's services.
pub struct TabServices(pub Rc<RefCell<TabCore>>);

impl Services for TabServices {
    fn request(&mut self, req: CapRequest, _out: &mut Vec<Delivery>) {
        let mut c = self.0.borrow_mut();
        match req.urn.as_str() {
            NET => c.net(req.args),
            STORE => c.store(&req.args),
            NAV => c.nav_req(&req.args),
            u if u == SHARD || u.starts_with("rho:gaze:shard/session/") => c.shard_req(u, req.args),
            _ => {
                if let Some(r) = req.args.last().and_then(as_name) {
                    c.push(Class::DomReply, r, err("revoked", ""));
                }
            }
        }
    }

    fn poll(&mut self, out: &mut Vec<Delivery>) {
        let mut c = self.0.borrow_mut();
        if let Ok(mut o) = c.async_out.lock() {
            out.append(&mut o);
        }
        if let Some(s) = &mut c.shard {
            for o in s.drain() {
                out.push(match o {
                    ShardOut::Reply { chan, datum } => Delivery::reply(Class::Shard, chan, datum),
                    ShardOut::BindSession { chan, label } => Delivery {
                        class: Class::Shard,
                        chan,
                        args: Vec::new(),
                        bind: Some(label),
                    },
                });
            }
        }
    }
}

/// Parse a hash in the forms pages and settings use.
pub fn parse_hash(s: &str) -> Option<[u8; 32]> {
    unhex(s.strip_prefix("blake2b-256:").unwrap_or(s))?
        .try_into()
        .ok()
}

#[cfg(test)]
mod tests;
