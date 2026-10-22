use alsa::device_name::HintIter;
use color_eyre::{
    Section,
    eyre::{self, Context, eyre},
};
use librespot_playback::mixer::{Mixer, MixerConfig};
use log::error;

pub fn get_available_controls() -> alsa::Result<HintIter> {
    alsa::device_name::HintIter::new_str(None, "ctl")
}

/// Opens the alsa mixer anew for every volume call (as spotifyd 0.3 did) instead of keeping the one opened at
/// startup: a mixer's elements are fixed when it is loaded, so a mixer opened while the device behind it is missing
/// (e.g. the `pulse` ctl plugin for a Bluetooth sink that is not connected yet, which reports 0 channels and never
/// sends an INFO event when the sink appears) would stay a silent no-op for the whole process lifetime.
pub struct AlsaMixer {
    pub config: MixerConfig,
}

impl AlsaMixer {
    fn open_mixer(&self) -> eyre::Result<alsa::Mixer> {
        alsa::Mixer::new(&self.config.device, false).wrap_err("failed to open mixer")
    }

    fn get_selem<'a>(&self, mixer: &'a alsa::Mixer) -> eyre::Result<alsa::mixer::Selem<'a>> {
        let selem_id = alsa::mixer::SelemId::new(&self.config.control, self.config.index);
        let selem = mixer.find_selem(&selem_id).ok_or_else(|| {
            eyre!(
                "No control with name '{}' in alsa device '{}'",
                self.config.control,
                self.config.device,
            )
            .with_suggestion(|| {
                format!(
                    "maybe try one of the following for 'control':{}",
                    mixer
                        .iter()
                        .filter_map(|elem| {
                            let selem = alsa::mixer::Selem::new(elem)?;
                            if selem.has_playback_volume() {
                                selem
                                    .get_id()
                                    .get_name()
                                    .ok()
                                    .map(|name| format!("\n- {name}"))
                            } else {
                                None
                            }
                        })
                        .collect::<String>()
                )
            })
        })?;
        Ok(selem)
    }

    fn set_volume_with_err(&self, volume: u16) -> eyre::Result<()> {
        let mixer = self.open_mixer()?;
        let elem = self.get_selem(&mixer)?;
        let (min, max) = elem.get_playback_volume_range();

        let volume_steps = (max - min) as f64;
        let normalised_volume = if matches!(
            self.config.volume_ctrl,
            librespot_playback::config::VolumeCtrl::Linear
        ) {
            (((volume as f64) / (u16::MAX as f64)) * volume_steps) as i64 + min
        } else {
            ((volume as f64 + 1.0).log((u16::MAX as f64) + 1.0) * volume_steps).floor() as i64 + min
        };

        elem.set_playback_volume_all(normalised_volume)?;
        Ok(())
    }

    fn get_volume_with_err(&self) -> eyre::Result<u16> {
        let mixer = self.open_mixer()?;
        let elem = self.get_selem(&mixer)?;
        let (min, max) = elem.get_playback_volume_range();
        let volume_steps = (max - min) as f64;
        let vol = elem.get_playback_volume(alsa::mixer::SelemChannelId::mono())?;
        let normalized_volume = if matches!(
            self.config.volume_ctrl,
            librespot_playback::config::VolumeCtrl::Linear
        ) {
            ((vol - min) as f64 * u16::MAX as f64 / volume_steps).floor() as u16
        } else {
            ((u16::MAX as f64 + 1.0).powf(((vol - min) as f64) / volume_steps) - 1.0).floor() as u16
        };
        Ok(normalized_volume)
    }
}

impl Mixer for AlsaMixer {
    fn open(config: MixerConfig) -> Result<AlsaMixer, librespot_core::Error> {
        // Fail early on a device that cannot be opened at all; the handle itself is not kept.
        alsa::Mixer::new(&config.device, false)
            .wrap_err("failed to open mixer")
            .map_err(librespot_core::Error::invalid_argument)?;
        Ok(AlsaMixer { config })
    }

    fn volume(&self) -> u16 {
        match self.get_volume_with_err() {
            Ok(vol) => vol,
            Err(err) => {
                error!("failed to get volume from alsa device: {err:?}");
                0
            }
        }
    }

    fn set_volume(&self, volume: u16) {
        match self.set_volume_with_err(volume) {
            Ok(_) => (),
            Err(err) => error!("Couldn't set volume of alsa device: {err:?}"),
        }
    }
}
