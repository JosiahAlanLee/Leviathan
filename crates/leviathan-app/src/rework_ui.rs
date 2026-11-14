use std::sync::Arc;
use std::thread::{self, JoinHandle};

use eframe::egui;
use leviathan_core::rack_routing::SampleRegion;
use leviathan_dsp::rework::{self, Params};

use crate::audition::{Request, Sample};
use crate::capture::Take;
use crate::kit::KitSample;
use crate::project::InstrumentSettings;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Target {
    #[default]
    Take,
    Kit(usize),
}

enum Source {
    Take(Arc<Take>, SampleRegion),
    Kit(Arc<KitSample>, f32),
}

#[derive(Default)]
pub struct Action {
    pub commit: bool,
    pub audition: Option<Request>,
}

enum ResultPcm {
    Take(Arc<Take>),
    Kit(Arc<KitSample>),
}

impl Source {
    fn mutate(self, params: Params) -> Result<(ResultPcm, [[f32; 64]; 2]), String> {
        match self {
            Self::Take(take, region) => {
                let mut output = take.frames.clone();
                let range = region.frames(output.len());
                let mut scratch = vec![[0.0; 3]; range.len()];
                if !rework::mutate(
                    &take.frames[range.clone()],
                    &mut output[range.clone()],
                    &mut scratch,
                    params,
                ) {
                    return Err("Could not rework the capture region".into());
                }
                let peaks = [
                    stereo_peaks(&take.frames[range.clone()]),
                    stereo_peaks(&output[range]),
                ];
                Take::new(take.rate, output)
                    .map(|take| (ResultPcm::Take(take), peaks))
                    .ok_or("Invalid reworked take".into())
            }
            Self::Kit(sample, start) => {
                let mut output = sample.pcm.clone();
                let first = ((output.len() - 1) as f32 * start) as usize;
                let first = first.min(output.len() - 2);
                let mut scratch = vec![[0.0; 3]; output.len() - first];
                let input = sample.pcm[first..].as_chunks::<1>().0;
                let dest = output[first..].as_chunks_mut::<1>().0;
                if !rework::mutate(input, dest, &mut scratch, params) {
                    return Err("Could not rework the kit slice".into());
                }
                let peaks = [
                    crate::wav::sample_peaks(&sample.pcm[first..]),
                    crate::wav::sample_peaks(&output[first..]),
                ];
                KitSample::new(sample.name.clone(), sample.rate, output)
                    .map(|sample| (ResultPcm::Kit(sample), peaks))
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Stamp {
    project: u64,
    target: Target,
    generation: u64,
    start: f32,
    end: f32,
}

impl Stamp {
    fn new(
        project: u64,
        target: Target,
        instruments: &InstrumentSettings,
    ) -> Option<(Self, Source)> {
        let (generation, start, end, source) = match target {
            Target::Take => {
                let take = instruments.take.as_ref()?;
                let region = instruments.routing.region;
                (
                    take.generation(),
                    region.trim_start,
                    region.trim_end,
                    Source::Take(take.clone(), region),
                )
            }
            Target::Kit(slot) => {
                let sample = instruments.kit.samples[slot].as_ref()?;
                let start = instruments.kit.params[slot].start;
                (
                    sample.generation(),
                    start,
                    1.0,
                    Source::Kit(sample.clone(), start),
                )
            }
        };
        Some((
            Self {
                project,
                target,
                generation,
                start,
                end,
            },
            source,
        ))
    }

    fn matches(self, project: u64, instruments: &InstrumentSettings) -> bool {
        if self.project != project {
            return false;
        }
        Self::new(project, self.target, instruments).is_some_and(|(current, _)| {
            current.generation == self.generation
                && current.start == self.start
                && current.end == self.end
        })
    }
}

struct Job {
    stamp: Stamp,
    revision: u64,
    worker: JoinHandle<Result<(ResultPcm, [[f32; 64]; 2]), String>>,
}

struct Preview {
    stamp: Stamp,
    pcm: ResultPcm,
    peaks: [[f32; 64]; 2],
}

#[derive(Default)]
pub struct ReworkUi {
    params: Params,
    target: Target,
    job: Option<Job>,
    preview: Option<Preview>,
    pub status: String,
    revision: u64,
}

impl ReworkUi {
    fn same_job(&self, project: u64, instruments: &InstrumentSettings) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|p| p.stamp.matches(project, instruments))
    }

    pub fn poll(&mut self, ctx: &egui::Context, project: u64, instruments: &InstrumentSettings) {
        if self.job.as_ref().is_some_and(|j| j.worker.is_finished()) {
            let job = self.job.take().unwrap();
            let result = job
                .worker
                .join()
                .unwrap_or_else(|_| Err("REWORK worker stopped".into()));
            if job.revision != self.revision || !job.stamp.matches(project, instruments) {
                self.status = "Source changed during REWORK".into();
            } else {
                match result {
                    Ok((pcm, peaks)) => {
                        self.preview = Some(Preview {
                            stamp: job.stamp,
                            pcm,
                            peaks,
                        });
                        self.status = "REWORK preview ready".into();
                    }
                    Err(error) => self.status = error,
                }
            }
        }
        if self.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
        if self.preview.is_some() && !self.same_job(project, instruments) {
            self.preview = None;
            self.status = "REWORK preview source changed".into();
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn reset(&mut self) {
        self.target = Target::Take;
        self.preview = None;
        self.status.clear();
    }

    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        project: u64,
        instruments: &InstrumentSettings,
        slot: usize,
        has_audio: bool,
    ) -> Action {
        let mut action = Action::default();
        egui::CollapsingHeader::new("REWORK").show(ui, |ui| {
            let selection = if matches!(self.target, Target::Kit(_)) {
                Target::Kit(slot)
            } else {
                Target::Take
            };
            let mut changed = selection != self.target;
            self.target = selection;
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .selectable_value(&mut self.target, Target::Take, "Capture region")
                    .changed();
                changed |= ui
                    .selectable_value(
                        &mut self.target,
                        Target::Kit(slot),
                        format!("Kit {}", slot + 1),
                    )
                    .changed();
            });
            changed |= ui
                .add(egui::DragValue::new(&mut self.params.seed).prefix("Seed "))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut self.params.smoothness, 0.0..=1.0).text("Fineness"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut self.params.amount, 0.0..=1.0).text("Amount"))
                .changed();
            if changed {
                self.preview = None;
                self.revision = self.revision.wrapping_add(1);
                if self.job.is_some() {
                    self.status = "Finishing previous REWORK".into();
                }
            }
            let eligible = Stamp::new(project, self.target, instruments);
            if ui
                .add_enabled(
                    self.job.is_none() && eligible.is_some(),
                    egui::Button::new("Preview REWORK"),
                )
                .clicked()
            {
                if let Some((stamp, source)) = eligible {
                    self.preview = None;
                    let params = self.params;
                    match thread::Builder::new()
                        .name("leviathan-rework".into())
                        .spawn(move || source.mutate(params))
                    {
                        Ok(worker) => {
                            self.job = Some(Job {
                                stamp,
                                revision: self.revision,
                                worker,
                            });
                            self.status = "REWORK processing".into();
                            ui.ctx().request_repaint();
                        }
                        Err(error) => self.status = error.to_string(),
                    }
                }
            }
            if let Some(preview) = self
                .preview
                .as_ref()
                .filter(|p| p.stamp.target == self.target)
            {
                draw_peaks(ui, "Source", preview.peaks[0]);
                draw_peaks(ui, "REWORK", preview.peaks[1]);
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(has_audio, |ui| {
                        let dry = ui.button("Listen A").clicked();
                        let wet = ui.button("Listen B").clicked();
                        let audible = (dry || wet).then_some(wet);
                        if let Some(wet) = audible {
                            action.audition = self.audition(instruments, wet);
                        }
                    });
                    action.commit = ui.button("Commit REWORK").clicked();
                });
            }
            if !self.status.is_empty() {
                ui.small(&self.status);
            }
        });
        action
    }

    fn audition(&self, instruments: &InstrumentSettings, wet: bool) -> Option<Request> {
        let preview = self.preview.as_ref()?;
        match (&preview.pcm, preview.stamp.target) {
            (ResultPcm::Take(processed), Target::Take) => {
                let take = if wet {
                    processed
                } else {
                    instruments.take.as_ref()?
                };
                Some(Request {
                    sample: Sample::Take(take.clone()),
                    range: instruments.routing.region.frames(take.frames.len()),
                    reverse: false,
                })
            }
            (ResultPcm::Kit(processed), Target::Kit(slot)) => {
                let sample = if wet {
                    processed
                } else {
                    instruments.kit.samples[slot].as_ref()?
                };
                Some(kit_audition(sample.clone(), instruments.kit.params[slot]))
            }
            _ => None,
        }
    }

    pub fn commit(&mut self, project: u64, instruments: &mut InstrumentSettings) -> bool {
        let Some(preview) = self
            .preview
            .take()
            .filter(|p| p.stamp.matches(project, instruments))
        else {
            return false;
        };
        match (preview.stamp.target, preview.pcm) {
            (Target::Take, ResultPcm::Take(take)) => instruments.take = Some(take),
            (Target::Kit(slot), ResultPcm::Kit(sample)) => {
                if let Err(error) = instruments.kit.replace(slot, sample) {
                    self.status = error;
                    return false;
                }
            }
            _ => return false,
        }
        self.status = "REWORK applied".into();
        true
    }

    pub fn target(&self) -> Target {
        self.target
    }
}

pub fn kit_audition(sample: Arc<KitSample>, slot: leviathan_dsp::otis_kit::Slot) -> Request {
    let first = ((sample.pcm.len() - 1) as f32 * slot.start) as usize;
    let end = sample.pcm.len();
    Request {
        sample: Sample::Kit(sample),
        range: first.min(end - 2)..end,
        reverse: slot.reverse,
    }
}

fn stereo_peaks(frames: &[[i16; 2]]) -> [f32; 64] {
    std::array::from_fn(|bin| {
        let start = frames.len() * bin / 64;
        let end = (frames.len() * (bin + 1) / 64)
            .max(start + 1)
            .min(frames.len());
        frames[start..end]
            .iter()
            .flatten()
            .map(|v| f32::from(v.unsigned_abs()) / 32768.0)
            .fold(0.0, f32::max)
    })
}

fn draw_peaks(ui: &mut egui::Ui, label: &str, peaks: [f32; 64]) {
    ui.small(label);
    let width = ui.available_width();
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 24.0), egui::Sense::hover());
    let rect = response.rect;
    for (i, peak) in peaks.into_iter().enumerate() {
        let x = rect.left() + rect.width() * i as f32 / 64.0;
        let dy = rect.height() * 0.5 * peak;
        painter.line_segment(
            [
                egui::pos2(x, rect.center().y - dy),
                egui::pos2(x, rect.center().y + dy),
            ],
            egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn committed_rework_preserves_the_trim_boundary_and_round_trips_as_pcm() {
        let context = eframe::CreationContext::_new_kittest(egui::Context::default());
        let app = crate::LeviathanApp::with_audio(&context, |_, _, _, _, _| Err("headless".into()));
        let mut project = app.project_snapshot();
        let original = Take::new(
            48_000,
            (0..1024)
                .map(|i| {
                    [
                        ((i * 391) % 14_109) as i16 - 7000,
                        ((i * 373) % 8111) as i16 - 4000,
                    ]
                })
                .collect(),
        )
        .unwrap();
        project.instruments.take = Some(original.clone());
        project.instruments.routing.region.trim_start = 0.25;
        project.instruments.routing.region.trim_end = 0.75;
        let (stamp, source) = Stamp::new(1, Target::Take, &project.instruments).unwrap();
        let (pcm, peaks) = source.mutate(Params::default()).unwrap();
        let mut ui = ReworkUi {
            preview: Some(Preview { stamp, pcm, peaks }),
            ..ReworkUi::default()
        };
        assert!(ui.commit(1, &mut project.instruments));
        let committed = project.instruments.take.clone().unwrap();
        assert_eq!(original.frames[..256], committed.frames[..256]);
        assert_eq!(original.frames[768..], committed.frames[768..]);
        assert_ne!(original.frames[256..768], committed.frames[256..768]);
        assert!(!ui.commit(1, &mut project.instruments));

        let path =
            std::env::temp_dir().join(format!("leviathan-rework-{}.json", std::process::id()));
        crate::project::save(&path, project).unwrap();
        let reopened = crate::project::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            reopened.instruments.take.as_ref().unwrap().frames,
            committed.frames
        );
    }

    #[test]
    fn stale_preview_cannot_replace_an_updated_slot_or_project() {
        let mut instruments = InstrumentSettings::default();
        instruments.take = Take::new(48_000, vec![[1400; 2]; 80]);
        let (stamp, source) = Stamp::new(4, Target::Take, &instruments).unwrap();
        let (pcm, peaks) = source.mutate(Params::default()).unwrap();
        let mut ui = ReworkUi {
            preview: Some(Preview { stamp, pcm, peaks }),
            ..ReworkUi::default()
        };
        instruments.take = Take::new(48_000, vec![[2400; 2]; 80]);
        assert!(!ui.commit(4, &mut instruments));
        assert_eq!(instruments.take.as_ref().unwrap().frames[0], [2400; 2]);
        instruments.kit.load_factory();
        let (stamp, source) = Stamp::new(4, Target::Kit(0), &instruments).unwrap();
        let (pcm, peaks) = source.mutate(Params::default()).unwrap();
        ui.preview = Some(Preview { stamp, pcm, peaks });
        assert!(!ui.commit(5, &mut instruments));
    }
}
