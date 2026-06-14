#[derive(Clone, Copy, PartialEq, defmt::Format, strum::IntoStaticStr)]
pub enum ReflowPhaseName {
    Preheat,
    Soak,
    Reflow,
}

#[derive(PartialEq)]
pub struct ReflowPhase {
    pub name: ReflowPhaseName,
    pub duration_secs: u16,
    pub end_temp: u16,
}

pub struct AlloyReflowProfile {
    pub alloy: &'static str,
    pub phases: &'static [ReflowPhase],
}

pub struct ReflowStatus {
    pub phase_name: ReflowPhaseName,
    pub target_temp: f32,
    pub time_left_in_phase: u16,
    pub total_time_left: u16,
}

impl AlloyReflowProfile {
    pub fn total_duration(&self) -> u16 {
        self.phases.iter().map(|p| p.duration_secs).sum()
    }

    fn phase_start_temp(&self, phase_index: usize, initial_temp: f32) -> f32 {
        if phase_index == 0 {
            initial_temp
        } else {
            self.phases[phase_index - 1].end_temp as f32
        }
    }

    // Unified function to get all current state data
    pub fn get_status(&self, elapsed_secs: u16, initial_temp: f32) -> Option<ReflowStatus> {
        let mut t_start = 0u16;
        let total_time = self.total_duration();

        for (i, phase) in self.phases.iter().enumerate() {
            let t_end = t_start + phase.duration_secs;

            // If we are currently inside this phase
            if elapsed_secs <= t_end {
                let offset = elapsed_secs - t_start;

                let start_temp = self.phase_start_temp(i, initial_temp) as f32;
                let end_temp = phase.end_temp as f32;

                let target_temp = start_temp
                    + ((end_temp - start_temp) * offset as f32) / phase.duration_secs as f32;

                return Some(ReflowStatus {
                    phase_name: phase.name,
                    target_temp: target_temp,
                    time_left_in_phase: phase.duration_secs - offset,
                    total_time_left: total_time.saturating_sub(elapsed_secs),
                });
            }
            t_start += phase.duration_secs;
        }

        // If elapsed_secs > total duration, the profile is complete
        None
    }
}

#[rustfmt::skip]
const SN42BI58_PHASES: &[ReflowPhase] = &[
    ReflowPhase { name: ReflowPhaseName::Preheat, duration_secs: 90, end_temp: 120 },
    ReflowPhase { name: ReflowPhaseName::Soak,    duration_secs: 60, end_temp: 138 },
    ReflowPhase { name: ReflowPhaseName::Reflow,  duration_secs: 30, end_temp: 165 },
];

pub static SN42BI58: AlloyReflowProfile = AlloyReflowProfile {
    alloy: "Sn42Bi58",
    phases: SN42BI58_PHASES,
};

#[rustfmt::skip]
const SN64BI35AG1_PHASES: &[ReflowPhase] = &[
    ReflowPhase { name: ReflowPhaseName::Preheat, duration_secs: 90, end_temp: 130 },
    ReflowPhase { name: ReflowPhaseName::Soak,    duration_secs: 60, end_temp: 150 },
    ReflowPhase { name: ReflowPhaseName::Reflow,  duration_secs: 30, end_temp: 185 },
];

pub static SN64BI35AG1: AlloyReflowProfile = AlloyReflowProfile {
    alloy: "Sn64Bi35Ag1",
    phases: SN64BI35AG1_PHASES,
};

pub static PROFILES: [&AlloyReflowProfile; 2] = [&SN42BI58, &SN64BI35AG1];
