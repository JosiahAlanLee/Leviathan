use std::sync::{
    Arc,
    atomic::{AtomicI32, Ordering},
};

use eframe::egui;
use jni::{JavaVM, jni_sig, jni_str, objects::{JObject, JValue}, refs::Global};
use winit::platform::android::activity::AndroidApp;

pub struct Orientation {
    app: AndroidApp,
    follow_rotation: Arc<AtomicI32>,
}

impl Orientation {
    pub fn new(app: AndroidApp, ctx: &egui::Context) -> Self {
        let this = Self { app, follow_rotation: Arc::new(AtomicI32::new(-1)) };
        this.request(-1, ctx);
        this
    }

    fn request(&self, desired: i32, ctx: &egui::Context) {
        self.follow_rotation.store(-1, Ordering::Release);
        let app = self.app.clone();
        let state = self.follow_rotation.clone();
        let ctx = ctx.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            let result = (|| -> jni::errors::Result<i32> {
                JavaVM::singleton()?.attach_current_thread(|env| -> jni::errors::Result<i32> {
                    let raw = app.activity_as_ptr() as jni::sys::jobject;
                    // The AndroidApp clone owns the activity during the call.
                    let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
                    env.call_method(activity.as_ref(), jni_str!("rotationPreference"),
                        jni_sig!("(I)I"), &[JValue::Int(desired)])?.i()
                })
            })();
            let value = match result {
                Ok(value @ (0 | 1)) => value,
                Ok(_) => -2,
                Err(error) => {
                    log::warn!("Could not set/read Android rotation preference: {error}");
                    -2
                }
            };
            state.store(value, Ordering::Release);
            ctx.request_repaint();
        }));
    }

    pub fn draw(&self, ui: &mut egui::Ui) {
        match self.follow_rotation.load(Ordering::Acquire) {
            value @ (0 | 1) => {
                let mut follow = value == 1;
                if ui.checkbox(&mut follow, "Follow device rotation")
                    .on_hover_text("Off keeps the portrait layout. On allows landscape when Android rotation is unlocked.")
                    .changed()
                {
                    self.request(if follow { 1 } else { 0 }, ui.ctx());
                }
            }
            -2 => {
                if ui.button("Retry rotation setting").clicked() { self.request(-1, ui.ctx()); }
            }
            _ => { ui.small("Reading rotation…"); }
        }
    }
}
