use serde::{Serialize, Deserialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ActuatorsState {
    pub rla: bool,
    pub rlb: bool,
    pub swpwr: bool,
    pub ina: bool,
    pub inb: bool,
}

impl Default for ActuatorsState {
    fn default() -> Self {
        Self {
            rla: false,
            rlb: false,
            swpwr: true, // Keep system 5V/3V powered by default
            ina: false,
            inb: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScheduledAction {
    pub datetime_utc: String, // format YYYY-MM-DDTHH:MM:SSZ
    pub state: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ScheduledActions {
    // Clé: "rla", "rlb", "swpwr", "ina", "inb"
    pub schedules: HashMap<String, Vec<ScheduledAction>>,
}

impl ScheduledActions {
    pub fn add_schedule(&mut self, actuator_id: &str, datetime_utc: String, state: bool) -> Result<(), String> {
        let list = self.schedules.entry(actuator_id.to_string()).or_insert_with(Vec::new);
        
        // Validation : max 3 planifications par actionneur
        if list.len() >= 3 {
            return Err("Limite de planifications atteinte (max 3 par actionneur).".to_string());
        }

        // Éliminer les doublons pour le même timestamp exact
        if list.iter().any(|s| s.datetime_utc == datetime_utc) {
            return Err("Une planification existe déjà à cette date et heure exacte.".to_string());
        }

        list.push(ScheduledAction { datetime_utc, state });
        
        // Trier par ordre chronologique (l'ordre lexicographique ISO8601 est chronologique)
        list.sort_by(|a, b| a.datetime_utc.cmp(&b.datetime_utc));

        Ok(())
    }
}

