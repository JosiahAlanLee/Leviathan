use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use eframe::egui;
use jni::{
    jni_sig, jni_str,
    objects::{JObject, JString, JValue},
    refs::Global,
    JavaVM,
};
use winit::platform::android::activity::AndroidApp;

#[derive(Clone, Copy)]
pub enum Kind {
    Wav = 1,
    Open = 2,
    Save = 3,
    ExportWav = 4,
    ExportFolder = 5,
}

pub enum Reply {
    File(PathBuf, String),
    Saved,
    Cancelled,
    Failed(String),
}

type Mailbox = Arc<Mutex<Option<Result<String, String>>>>;

pub struct Files {
    app: AndroidApp,
    pending: Option<Kind>,
    replies: Mailbox,
    last_check: Option<Instant>,
    recovery: Option<PathBuf>,
    recovering: bool,
    export_stage: Option<crate::export::MobileStage>,
    pub status: String,
}

impl Files {
    pub fn new(app: AndroidApp) -> Self {
        let recovery = app.internal_data_path().map(|dir| dir.join("files-recovery.leviathan.json"));
        let recovering = recovery.as_ref().is_some_and(|path| path.is_file());
        let status = if recovering {
            "Android Files was interrupted. Check any output in Files. Restore the pre-picker project or dismiss.".into()
        } else {
            String::new()
        };
        Self {
            app,
            pending: None,
            replies: Default::default(),
            last_check: None,
            recovery,
            recovering,
            export_stage: None,
            status,
        }
    }

    pub fn pending(&self) -> bool {
        self.pending.is_some()
    }


    pub fn recovering(&self) -> bool {
        self.recovering
    }

    pub fn recover_project(&self) -> Result<crate::project::Project, String> {
        crate::project::load(self.recovery.as_deref().ok_or("Recovery path unavailable")?)
    }

    pub fn clear_recovery(&mut self) -> Result<(), String> {
        if let Some(path) = &self.recovery {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("Could not remove recovery copy: {e}")),
            }
        }
        self.recovering = false;
        Ok(())
    }

    pub fn export_root(&self) -> Option<PathBuf> {
        self.app.internal_data_path().map(|dir| dir.join("wav-staging"))
    }

    pub fn launch(
        &mut self,
        kind: Kind,
        project: crate::project::Project,
        stage: Option<crate::export::MobileStage>,
        ctx: &egui::Context,
    ) -> Result<(), String> {
        if self.pending() || self.recovering() {
            return Err("Finish the current Files operation or recovery first.".into());
        }
        let recovery = self.recovery.as_ref().ok_or("Recovery path unavailable")?;
        let payload = match kind {
            Kind::Save => recovery.as_os_str(),
            Kind::ExportWav | Kind::ExportFolder => stage.as_ref()
                .ok_or("Rendered WAV is unavailable")?.root().as_os_str(),
            Kind::Open | Kind::Wav => std::ffi::OsStr::new(""),
        }.to_str().ok_or("Invalid private file path")?.to_owned();
        // Atomically retain one validated snapshot before Android can stop this activity.
        crate::project::save(recovery, project)?;
        self.export_stage = stage;
        self.pending = Some(kind);
        self.last_check = None;
        self.status = "Waiting for Android Files…".into();
        let app = self.app.clone();
        let replies = self.replies.clone();
        let ctx = ctx.clone();
        self.app.run_on_java_main_thread(Box::new(move || {
            let launch = || -> jni::errors::Result<()> {
                JavaVM::singleton()?.attach_current_thread(|env| -> jni::errors::Result<()> {
                    let raw = app.activity_as_ptr() as jni::sys::jobject;
                    // The cloned AndroidApp owns the Java activity for this callback.
                    let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
                    let data = env.new_string(&payload)?;
                    env.call_method(
                        activity.as_ref(),
                        jni_str!("launchDocument"),
                        jni_sig!("(ILjava/lang/String;)V"),
                        &[JValue::Int(kind as i32), JValue::Object(data.as_ref())],
                    )?;
                    Ok(())
                })
            };
            if let Err(error) = launch() {
                *replies.lock().unwrap_or_else(|e| e.into_inner()) = Some(Err(error.to_string()));
            }
            ctx.request_repaint();
        }));
        Ok(())
    }

    pub fn finish(&mut self) {
        drop(self.export_stage.take());
        if let Err(error) = self.clear_recovery() {
            self.recovering = true;
            self.status = error;
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Option<(Kind, Reply)> {
        let kind = self.pending?;
        if let Some(reply) = self
            .replies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            self.pending = None;
            let reply = match reply {
                Ok(s) if s == "cancel" => Reply::Cancelled,
                Ok(s) if s == "saved" => Reply::Saved,
                Ok(s) if s.starts_with("file\n") => s[5..]
                    .split_once('\n')
                    .map(|(path, name)| Reply::File(path.into(), name.into()))
                    .unwrap_or_else(|| Reply::Failed("Invalid document result.".into())),
                Ok(s) => Reply::Failed(s.strip_prefix("error\n").unwrap_or(&s).to_owned()),
                Err(error) => Reply::Failed(error),
            };
            return Some((kind, reply));
        }
        const DELAY: Duration = Duration::from_millis(180);
        let now = Instant::now();
        if self
            .last_check
            .is_none_or(|last| now.duration_since(last) >= DELAY)
        {
            self.last_check = Some(now);
            let app = self.app.clone();
            let replies = self.replies.clone();
            let ctx = ctx.clone();
            self.app
                .run_on_java_main_thread(Box::new(move || match read_result(&app) {
                    Ok(None) => {}
                    value => {
                        *replies.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                            value
                                .map(|result| result.unwrap_or_default())
                                .map_err(|e| e.to_string()),
                        );
                        ctx.request_repaint();
                    }
                }));
        }
        ctx.request_repaint_after(DELAY);
        None
    }
}

fn read_result(app: &AndroidApp) -> jni::errors::Result<Option<String>> {
    JavaVM::singleton()?.attach_current_thread(|env| -> jni::errors::Result<_> {
        let raw = app.activity_as_ptr() as jni::sys::jobject;
        // The AndroidApp owns the Activity global reference.
        let activity = unsafe { env.as_cast_raw::<Global<JObject>>(&raw)? };
        let result = env
            .call_method(
                activity.as_ref(),
                jni_str!("takeDocumentResult"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        if result.is_null() {
            return Ok(None);
        }
        let result = env.cast_local::<JString>(result)?;
        Ok(Some(result.try_to_string(env)?))
    })
}
