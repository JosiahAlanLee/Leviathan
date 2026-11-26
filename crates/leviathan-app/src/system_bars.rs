use eframe::egui;

/// Pixel distances Android reserves around the native surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Edges {
    pub fn reserve(self, ctx: &egui::Context) {
        let scale = ctx.pixels_per_point();
        let points = |px: i32| px.max(0) as f32 / scale;
        let fill = ctx.style().visuals.panel_fill;
        let bar = || egui::Frame::new().fill(fill);
        // Reserve from the outside in, before the app creates any panels.
        for (panel, px) in [
            (egui::TopBottomPanel::top("system_top"), self.top),
            (egui::TopBottomPanel::bottom("system_bottom"), self.bottom),
        ] {
            let height = points(px).min(ctx.available_rect().height().max(0.0));
            if height > 0.0 {
                panel.frame(bar()).exact_height(height).show(ctx, |ui| {
                    ui.set_min_height(height);
                });
            }
        }
        for (panel, px) in [
            (egui::SidePanel::left("system_left"), self.left),
            (egui::SidePanel::right("system_right"), self.right),
        ] {
            let width = points(px).min(ctx.available_rect().width().max(0.0));
            if width > 0.0 {
                panel
                    .frame(bar())
                    .resizable(false)
                    .exact_width(width)
                    .show(ctx, |ui| {
                        ui.set_min_width(width);
                    });
            }
        }
    }
}

#[cfg(target_os = "android")]
mod android {
    use super::Edges;
    use eframe::egui;
    use jni::{
        jni_sig, jni_str,
        objects::{JObject, JValue},
        refs::Global,
        JavaVM,
    };
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };
    use winit::platform::android::activity::AndroidApp;

    const REFRESH: Duration = Duration::from_millis(500);

    pub struct SystemBars {
        app: AndroidApp,
        snapshot: Arc<Mutex<Option<Edges>>>,
        sampled_at: Option<Instant>,
        last_size: egui::Vec2,
    }

    impl SystemBars {
        pub fn new(app: AndroidApp) -> Self {
            Self {
                app,
                snapshot: Arc::new(Mutex::new(None)),
                sampled_at: None,
                last_size: egui::Vec2::ZERO,
            }
        }

        pub fn update(&mut self, ctx: &egui::Context) -> Edges {
            let now = Instant::now();
            let size = ctx.screen_rect().size();
            if self.last_size != size
                || self
                    .sampled_at
                    .is_none_or(|t| now.duration_since(t) >= REFRESH)
            {
                self.last_size = size;
                self.sampled_at = Some(now);
                let app = self.app.clone();
                let snapshot = self.snapshot.clone();
                let ctx = ctx.clone();
                self.app.run_on_java_main_thread(Box::new(move || {
                    match read(&app) {
                        Ok(Some(edges)) => {
                            let mut seen = snapshot.lock().unwrap_or_else(|e| e.into_inner());
                            if *seen != Some(edges) {
                                *seen = Some(edges);
                                ctx.request_repaint();
                            }
                        }
                        Ok(None) => {} // Decor view not attached yet; retry on a later frame.
                        Err(error) => log::warn!("Could not read system bar insets: {error}"),
                    }
                }));
            }
            // Detect bar visibility changes even when the transport is stopped.
            ctx.request_repaint_after(REFRESH);
            self.snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unwrap_or_default()
        }
    }

    fn read(app: &AndroidApp) -> jni::errors::Result<Option<Edges>> {
        if AndroidApp::sdk_version() < 35 {
            return Ok(Some(Edges::default()));
        }
        JavaVM::singleton()?.attach_current_thread(|env| -> jni::errors::Result<_> {
            let ptr = app.activity_as_ptr() as jni::sys::jobject;
            // AndroidApp owns this global JNI reference for the callback lifetime.
            let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&ptr)? };
            let window = env
                .call_method(
                    activity.as_ref(),
                    jni_str!("getWindow"),
                    jni_sig!("()Landroid/view/Window;"),
                    &[],
                )?
                .l()?;
            if window.is_null() {
                return Ok(None);
            }
            let decor = env
                .call_method(
                    &window,
                    jni_str!("peekDecorView"),
                    jni_sig!("()Landroid/view/View;"),
                    &[],
                )?
                .l()?;
            if decor.is_null() {
                return Ok(None);
            }
            let insets = env
                .call_method(
                    &decor,
                    jni_str!("getRootWindowInsets"),
                    jni_sig!("()Landroid/view/WindowInsets;"),
                    &[],
                )?
                .l()?;
            if insets.is_null() {
                return Ok(None);
            }

            let ty = env.find_class(jni_str!("android/view/WindowInsets$Type"))?;
            let bars = env
                .call_static_method(&ty, jni_str!("systemBars"), jni_sig!("()I"), &[])?
                .i()?;
            let cutout = env
                .call_static_method(&ty, jni_str!("displayCutout"), jni_sig!("()I"), &[])?
                .i()?;
            let px = env
                .call_method(
                    &insets,
                    jni_str!("getInsets"),
                    jni_sig!("(I)Landroid/graphics/Insets;"),
                    &[JValue::Int(bars | cutout)],
                )?
                .l()?;
            Ok(Some(Edges {
                left: env.get_field(&px, jni_str!("left"), jni_sig!("I"))?.i()?,
                top: env.get_field(&px, jni_str!("top"), jni_sig!("I"))?.i()?,
                right: env.get_field(&px, jni_str!("right"), jni_sig!("I"))?.i()?,
                bottom: env.get_field(&px, jni_str!("bottom"), jni_sig!("I"))?.i()?,
            }))
        })
    }
}

#[cfg(target_os = "android")]
pub use android::SystemBars;
