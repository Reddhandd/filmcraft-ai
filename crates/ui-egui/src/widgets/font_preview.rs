//! Shared font-name rows: bounded temporary rendering, worker queues and texture cache.
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use web_time::{Duration, Instant};

const CACHE_LIMIT: usize = 32;
#[cfg(not(target_arch = "wasm32"))]
const QUEUE_LIMIT: usize = 8;
const FRAME_WORK: usize = 4;
const DEADLINE: Duration = Duration::from_secs(10);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    family: String,
    scale: u16,
}
impl Key {
    fn new(ctx: &egui::Context, family: &str) -> Option<Self> {
        if family.is_empty() || family.len() > 512 {
            return None;
        }
        let ppp = ctx.pixels_per_point();
        let ppp = if ppp.is_finite() && ppp > 0.0 { ppp.clamp(0.75, 4.0) } else { 1.0 };
        Some(Self { family: family.into(), scale: (ppp * 16.0).round() as u16 })
    }
}
#[cfg(not(target_arch = "wasm32"))]
enum Job {
    Discover,
    Render(u64, Key),
}
enum Event {
    #[cfg(not(target_arch = "wasm32"))]
    Discovered(bool),
    Rendered(u64, Instant, Result<egui::ColorImage, ()>),
}
struct Worker {
    #[cfg(not(target_arch = "wasm32"))]
    jobs: mpsc::SyncSender<Job>,
    events: mpsc::Receiver<Event>,
}
#[cfg(not(target_arch = "wasm32"))]
impl Worker {
    fn start() -> Result<Self, ()> {
        let (jobs, requests) = mpsc::sync_channel(QUEUE_LIMIT);
        let (results, events) = mpsc::sync_channel(QUEUE_LIMIT);
        std::thread::Builder::new()
            .name("font-picker-previews".into())
            .spawn(move || {
                let guarded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while let Ok(job) = requests.recv() {
                        let event = match job {
                            Job::Discover => Event::Discovered(std::panic::catch_unwind(filmcraft_text::fonts::scan_system).is_ok()),
                            Job::Render(id, key) => {
                                let result = std::panic::catch_unwind(|| render(&key)).unwrap_or(Err(()));
                                Event::Rendered(id, Instant::now(), result)
                            }
                        };
                        if results.send(event).is_err() {
                            break;
                        }
                    }
                }));
                if guarded.is_err() {
                    log::warn!(target:"filmcraft::fonts","Font preview worker failed; picker remains available");
                }
            })
            .map_err(|_| ())?;
        // No Context is captured by the worker: dropping the cache closes its channels.
        jobs.try_send(Job::Discover).map_err(|_| ())?;
        Ok(Self { jobs, events })
    }
}
fn render(key: &Key) -> Result<egui::ColorImage, ()> {
    if !(12..=64).contains(&key.scale) {
        return Err(());
    }
    let scale = f32::from(key.scale) / 16.0;
    let width = (240.0 * scale).ceil() as usize;
    let height = (24.0 * scale).ceil() as usize;
    let mask = filmcraft_text::preview::family(&key.family, 16.0 * scale, width, height).map_err(|_| ())?;
    let count = mask.w.checked_mul(mask.h).filter(|count| *count == mask.a.len() && *count <= 960 * 96).ok_or(())?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(count).map_err(|_| ())?;
    for value in mask.a {
        let alpha = if value.is_finite() { (value.clamp(0.0, 1.0) * 255.0).round() as u8 } else { 0 };
        pixels.push(egui::Color32::from_white_alpha(alpha));
    }
    Ok(egui::ColorImage::new([mask.w, mask.h], pixels))
}
enum Image {
    Pending(Instant),
    Ready(egui::TextureHandle),
    Unavailable,
}
struct Entry {
    id: u64,
    key: Key,
    image: Image,
}
#[derive(Default)]
struct Cache {
    worker: Option<Worker>,
    started: bool,
    dead: bool,
    catalog_pending: Option<Instant>,
    catalog_failed: bool,
    entries: VecDeque<Entry>,
    next: u64,
    frame: u64,
    received: usize,
    requested: usize,
}
impl Cache {
    fn start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        #[cfg(not(target_arch = "wasm32"))]
        match Worker::start() {
            Ok(worker) => {
                self.worker = Some(worker);
                self.catalog_pending = Some(Instant::now());
            }
            Err(()) => {
                self.dead = true;
                self.catalog_failed = true;
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            filmcraft_text::fonts::scan_system();
        }
    }
    fn apply(&mut self, ctx: &egui::Context, event: Event) {
        match event {
            #[cfg(not(target_arch = "wasm32"))]
            Event::Discovered(ok) => {
                self.catalog_pending = None;
                self.catalog_failed = !ok;
                ctx.request_repaint();
            }
            Event::Rendered(id, finished, result) => {
                let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) else {
                    return;
                };
                let Image::Pending(since) = entry.image else {
                    return;
                };
                if finished.saturating_duration_since(since) > DEADLINE {
                    entry.image = Image::Unavailable;
                    return;
                }
                let limit = ctx.input(|input| input.max_texture_side);
                entry.image = match result {
                    Ok(image) if valid_image(&image, limit) => Image::Ready(ctx.load_texture("font-name-preview", image, egui::TextureOptions::LINEAR)),
                    _ => Image::Unavailable,
                };
            }
        }
    }
    fn step(&mut self, ctx: &egui::Context) {
        self.start();
        let frame = ctx.cumulative_frame_nr();
        if self.frame != frame {
            self.frame = frame;
            self.received = 0;
            self.requested = 0;
        }
        while self.received < FRAME_WORK {
            let event = self.worker.as_ref().map(|worker| worker.events.try_recv());
            match event {
                Some(Ok(event)) => {
                    self.received = self.received.saturating_add(1);
                    self.apply(ctx, event);
                }
                Some(Err(mpsc::TryRecvError::Disconnected)) => {
                    self.dead = true;
                    self.worker = None;
                    self.catalog_failed = true;
                    self.catalog_pending = None;
                    break;
                }
                _ => break,
            }
        }
        // Drain ready results across bounded frames before declaring an old request stuck.
        // Completion timestamps distinguish slow work from an app that was merely hidden.
        if self.received < FRAME_WORK {
            self.expire(Instant::now());
        }
        if self.catalog_pending.is_some() || self.entries.iter().any(|entry| matches!(entry.image, Image::Pending(_))) {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }
    fn expire(&mut self, now: Instant) {
        let stuck = self.catalog_pending.is_some_and(|since| now.saturating_duration_since(since) > DEADLINE)
            || self.entries.iter().any(|entry| matches!(entry.image,Image::Pending(since) if now.saturating_duration_since(since)>DEADLINE));
        if stuck {
            self.dead = true;
            self.worker = None;
            self.catalog_failed = self.catalog_pending.is_some() || self.catalog_failed;
            self.catalog_pending = None;
        }
        for entry in &mut self.entries {
            if self.dead && matches!(entry.image, Image::Pending(_)) {
                entry.image = Image::Unavailable;
            }
        }
    }
    fn image(&mut self, ctx: &egui::Context, key: Key) -> (Option<egui::TextureHandle>, &'static str) {
        self.step(ctx);
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            let Some(entry) = self.entries.remove(index) else {
                return (None, "Font preview unavailable");
            };
            let output = match &entry.image {
                Image::Ready(texture) => (Some(texture.clone()), ""),
                Image::Pending(_) => (None, "Loading font preview…"),
                Image::Unavailable => (None, "Preview unavailable; this font can still be selected"),
            };
            self.entries.push_back(entry);
            return output;
        }
        if self.dead {
            return (None, "Preview worker unavailable; this font can still be selected");
        }
        #[cfg(not(target_arch = "wasm32"))]
        let budget = FRAME_WORK;
        #[cfg(target_arch = "wasm32")]
        let budget = 1;
        if self.requested >= budget {
            ctx.request_repaint_after(Duration::from_millis(50));
            return (None, "Loading font preview…");
        }
        let Some(id) = self.next.checked_add(1) else {
            return (None, "Font preview counter exhausted");
        };
        self.next = id;
        self.requested = self.requested.saturating_add(1);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let Some(worker) = &self.worker else {
                return (None, "Font preview unavailable");
            };
            if worker.jobs.try_send(Job::Render(id, key.clone())).is_err() {
                ctx.request_repaint_after(Duration::from_millis(50));
                return (None, "Loading font preview…");
            }
        }
        if self.entries.len() >= CACHE_LIMIT {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry { id, key: key.clone(), image: Image::Pending(Instant::now()) });
        #[cfg(target_arch = "wasm32")]
        {
            let result = render(&key);
            self.apply(ctx, Event::Rendered(id, Instant::now(), result));
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        (None, "Loading font preview…")
    }
}
fn valid_image(image: &egui::ColorImage, limit: usize) -> bool {
    let [width, height] = image.size;
    width > 0 && height > 0 && width <= 960 && height <= 96 && width <= limit && height <= limit && width.checked_mul(height) == Some(image.pixels.len())
}
fn shared(ctx: &egui::Context) -> Arc<Mutex<Cache>> {
    ctx.data_mut(|data| data.get_temp_mut_or_default::<Arc<Mutex<Cache>>>(egui::Id::unique("font-picker-previews")).clone())
}
/// Call only inside an open popup. System discovery runs on the same bounded native worker.
pub(crate) fn prepare(ui: &mut egui::Ui) {
    let cache = shared(ui.ctx());
    let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
    cache.step(ui.ctx());
    if filmcraft_text::fonts::discovery_limited() {
        ui.small(tl!("Some fonts were omitted because a discovery safety limit was reached."));
    }
    if cache.dead {
        ui.small(tl!("Font previews unavailable; listed fonts remain selectable."));
    } else if cache.catalog_pending.is_some() {
        ui.small(tl!("Loading system fonts…"));
    } else if cache.catalog_failed {
        ui.small(tl!("System font discovery unavailable; existing fonts remain selectable."));
    }
}
/// Bound visible/accessibility text even for malformed font name tables.
pub(crate) fn label(family: &str) -> String {
    let label: String = family.chars().take(80).filter(|character| !character.is_control()).collect();
    if label.trim().is_empty() { "Unnamed font".into() } else { label }
}
/// Preserve ordinary legacy aliases, with a disjoint namespace for oversized names.
pub(crate) fn suffix(family: &str, index: usize) -> String {
    if family.len() <= 512 && !family.starts_with("@long/") { family.into() } else { format!("@long/{index}") }
}
/// A selectable row with an accessible family name and actual-typeface pixels when available.
pub(crate) fn row(ui: &mut egui::Ui, selected: bool, family: &str) -> egui::Response {
    let label = label(family);
    let width = ui.available_width();
    let width = if width.is_finite() { width.clamp(240.0, 400.0) } else { 240.0 };
    let response = ui.add(egui::Button::new("").selected(selected).min_size(egui::vec2(width, 28.0)));
    let response = super::accessible_name(response, label.clone());
    let mut hint = "Font preview unavailable";
    if ui.is_rect_visible(response.rect) {
        let image = Key::new(ui.ctx(), family).and_then(|key| {
            let cache = shared(ui.ctx());
            let mut cache = cache.lock().unwrap_or_else(PoisonError::into_inner);
            let (image, status) = cache.image(ui.ctx(), key);
            hint = status;
            image
        });
        let rect = response.rect.shrink(3.0);
        let color = if selected { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() };
        if let Some(image) = image {
            let scale = Key::new(ui.ctx(), family).map_or(1.0, |key| f32::from(key.scale) / 16.0);
            let size = image.size_vec2() / scale;
            let target = egui::Rect::from_min_size(egui::pos2(rect.left(), rect.center().y - size.y * 0.5), size);
            ui.painter().with_clip_rect(rect).image(image.id(), target, egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)), color);
        } else {
            ui.painter().with_clip_rect(rect).text(rect.left_center(), egui::Align2::LEFT_CENTER, &label, egui::FontId::proportional(13.0), color);
        }
    }
    if hint.is_empty() { response.on_hover_text(label) } else { response.on_hover_text(format!("{label}\n{hint}")) }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    fn key(name: &str) -> Key {
        Key { family: name.into(), scale: 16 }
    }
    fn fake() -> (Cache, mpsc::Receiver<Job>, mpsc::SyncSender<Event>) {
        let (jobs, requests) = mpsc::sync_channel(QUEUE_LIMIT);
        let (results, events) = mpsc::sync_channel(QUEUE_LIMIT);
        (Cache { worker: Some(Worker { jobs, events }), started: true, ..Default::default() }, requests, results)
    }
    #[test]
    fn font_preview_labels_and_ids_are_bounded_and_disjoint() {
        assert_eq!(label("\0\n\t "), "Unnamed font");
        assert_eq!(label(&"🙂".repeat(10000)).chars().count(), 80);
        assert_eq!(suffix("Inter", 1), "Inter");
        assert_eq!(suffix(&"x".repeat(10000), 2), "@long/2");
        assert_eq!(suffix("@long/2", 3), "@long/3");
    }
    #[test]
    fn font_preview_completion_time_and_identity_guard_texture_uploads() {
        let ctx = egui::Context::default();
        let start = Instant::now();
        let (mut cache, _requests, _results) = fake();
        cache.entries.push_back(Entry { id: 1, key: key("Inter"), image: Image::Pending(start) });
        let image = egui::ColorImage::filled([1, 1], egui::Color32::WHITE);
        cache.apply(&ctx, Event::Rendered(99, start, Ok(image.clone())));
        assert!(matches!(cache.entries.front().unwrap().image, Image::Pending(_)));
        cache.apply(&ctx, Event::Rendered(1, start + Duration::from_secs(1), Ok(image.clone())));
        assert!(matches!(cache.entries.front().unwrap().image, Image::Ready(_)));
        cache.entries.clear();
        cache.entries.push_back(Entry { id: 2, key: key("Inter"), image: Image::Pending(start) });
        cache.apply(&ctx, Event::Rendered(2, start + DEADLINE + Duration::from_secs(1), Ok(image)));
        assert!(matches!(cache.entries.front().unwrap().image, Image::Unavailable));
    }
    #[test]
    fn font_preview_stalled_or_disconnected_workers_stop_pending_states() {
        let ctx = egui::Context::default();
        let start = Instant::now();
        let (mut cache, _requests, _results) = fake();
        cache.entries.push_back(Entry { id: 1, key: key("Inter"), image: Image::Pending(start) });
        cache.expire(start + DEADLINE + Duration::from_secs(1));
        assert!(cache.dead && cache.worker.is_none());
        assert!(matches!(cache.entries.front().unwrap().image, Image::Unavailable));
        let (mut cache, _requests, results) = fake();
        drop(results);
        cache.entries.push_back(Entry { id: 1, key: key("Inter"), image: Image::Pending(start) });
        cache.step(&ctx);
        assert!(cache.dead);
        assert!(matches!(cache.entries.front().unwrap().image, Image::Unavailable));
    }
    #[test]
    fn font_preview_cache_stays_bounded_while_browsing() {
        let ctx = egui::Context::default();
        let (mut cache, requests, results) = fake();
        for index in 0..100 {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let _ = cache.image(ui.ctx(), key(&format!("Family {index}")));
            });
            output.textures_delta.clear();
            for job in requests.try_iter() {
                if let Job::Render(id, _) = job {
                    results.try_send(Event::Rendered(id, Instant::now(), Err(()))).unwrap();
                }
            }
            assert!(cache.entries.len() <= CACHE_LIMIT);
            assert!(cache.requested <= FRAME_WORK);
        }
        assert_eq!(cache.entries.len(), CACHE_LIMIT);
    }
    #[test]
    fn font_preview_rejects_bad_backing_pixels_and_device_limits() {
        let mut image = egui::ColorImage::filled([1, 1], egui::Color32::WHITE);
        assert!(valid_image(&image, 1));
        assert!(!valid_image(&image, 0));
        image.pixels.clear();
        assert!(!valid_image(&image, 2048));
        image.size = [usize::MAX, usize::MAX];
        assert!(!valid_image(&image, usize::MAX));
    }
    #[test]
    fn font_preview_row_remains_accessible_and_selectable_with_real_pixels() {
        use egui_kittest::Harness;
        use egui_kittest::kittest::Queryable;
        let mut harness = Harness::builder().with_size(egui::vec2(300.0, 70.0)).build_ui_state(
            |ui, selected: &mut bool| {
                let key = Key::new(ui.ctx(), "Inter").unwrap();
                let cache = shared(ui.ctx());
                {
                    let mut cache = cache.lock().unwrap();
                    if !cache.started {
                        cache.started = true;
                        let texture = ui.ctx().load_texture("font-preview-row-test", render(&key).unwrap(), egui::TextureOptions::LINEAR);
                        cache.entries.push_back(Entry { id: 1, key, image: Image::Ready(texture) });
                    }
                }
                if row(ui, *selected, "Inter").clicked() {
                    *selected = !*selected;
                }
            },
            false,
        );
        harness.run_steps(2);
        harness.check_accessibility();
        harness.get_by_label("Inter").click();
        harness.run_steps(2);
        assert!(*harness.state());
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn font_preview_native_worker_returns_actual_bounded_font_pixels() {
        let worker = Worker::start().unwrap();
        worker.jobs.try_send(Job::Render(17, key("Inter"))).unwrap_or_else(|_| panic!("initial render queue full"));
        let mut found = false;
        for _ in 0..2 {
            match worker.events.recv_timeout(DEADLINE).unwrap() {
                Event::Discovered(ok) => assert!(ok),
                Event::Rendered(id, _, result) => {
                    assert_eq!(id, 17);
                    let image = result.unwrap();
                    assert!(valid_image(&image, 4096));
                    assert!(image.pixels.iter().any(|pixel| pixel.a() > 0));
                    found = true;
                }
            }
        }
        assert!(found);
    }
}
