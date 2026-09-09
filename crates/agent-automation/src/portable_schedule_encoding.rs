//! Closed JSONL records and an exact-byte footer detect damage, not trusted authorship.
use crate::{PackageError, PortableSchedulePackage};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PortableRecord<TTarget, TEndpoint> {
    PackageHeader {
        format: String,
        version: u32,
    },
    InstructionDocument {
        instruction_id: crate::InstructionId,
        text: crate::InstructionText,
        source_revision_id: String,
    },
    ScheduleDefinition {
        schedule_id: crate::ScheduleId,
        source_change_id: String,
        definition: crate::ScheduleDefinition<TTarget, TEndpoint>,
    },
    ContinuitySummary {
        text: crate::InstructionText,
        source_run_id: String,
        source_target: TTarget,
    },
    PackageEnd {
        record_count: u32,
        sha256: String,
    },
}
pub fn encode_schedule_package<TTarget: Clone + Serialize, TEndpoint: Clone + Serialize>(
    package: &PortableSchedulePackage<TTarget, TEndpoint>,
    maximum_bytes: usize,
) -> Result<String, PackageError> {
    let mut definition = package.definition.clone();
    definition.enabled = false;
    definition.destination = crate::ExecutionDestination::Unprepared;
    let mut records: Vec<PortableRecord<TTarget, TEndpoint>> = vec![
        PortableRecord::PackageHeader {
            format: "agentSchedule".into(),
            version: 1,
        },
        PortableRecord::InstructionDocument {
            instruction_id: package.instruction.instruction_id.clone(),
            text: package.instruction.text.clone(),
            source_revision_id: package.instruction.source_revision_id.clone(),
        },
        PortableRecord::ScheduleDefinition {
            schedule_id: package.schedule_id.clone(),
            source_change_id: package.source_change_id.clone(),
            definition,
        },
    ];
    if let Some(summary) = &package.continuity {
        records.push(PortableRecord::ContinuitySummary {
            text: summary.text.clone(),
            source_run_id: summary.source_run_id.clone(),
            source_target: summary.source_target.clone(),
        });
    }
    let mut result = String::new();
    for record in &records {
        result.push_str(&serde_json::to_string(record).map_err(|_| PackageError::InvalidRecord)?);
        result.push('\n');
    }
    let footer = PortableRecord::<TTarget, TEndpoint>::PackageEnd {
        record_count: u32::try_from(records.len()).map_err(|_| PackageError::InvalidRecord)?,
        sha256: format!("{:x}", Sha256::digest(result.as_bytes())),
    };
    result.push_str(&serde_json::to_string(&footer).map_err(|_| PackageError::InvalidRecord)?);
    result.push('\n');
    if result.len() > maximum_bytes {
        return Err(PackageError::TooLarge);
    }
    Ok(result)
}
pub fn decode_schedule_package<TTarget: DeserializeOwned, TEndpoint: DeserializeOwned>(
    text: &str,
    maximum_bytes: usize,
) -> Result<PortableSchedulePackage<TTarget, TEndpoint>, PackageError> {
    if text.len() > maximum_bytes {
        return Err(PackageError::TooLarge);
    }
    if !text.ends_with('\n') {
        return Err(PackageError::InvalidEncoding);
    }
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    if !(4..=5).contains(&lines.len())
        || lines
            .iter()
            .any(|line| line.ends_with("\r\n") || line.trim().is_empty())
    {
        return Err(PackageError::InvalidEncoding);
    }
    let mut records = lines
        .iter()
        .map(|line| {
            serde_json::from_str::<PortableRecord<TTarget, TEndpoint>>(line)
                .map_err(|_| PackageError::InvalidRecord)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let footer = records.pop().ok_or(PackageError::InvalidEncoding)?;
    let PortableRecord::PackageEnd {
        record_count,
        sha256,
    } = footer
    else {
        return Err(PackageError::InvalidEncoding);
    };
    let content = lines
        .iter()
        .take(records.len())
        .copied()
        .collect::<String>();
    if usize::try_from(record_count).ok() != Some(records.len())
        || sha256 != format!("{:x}", Sha256::digest(content.as_bytes()))
    {
        return Err(PackageError::IntegrityMismatch);
    }
    let mut records = records.into_iter();
    match records.next() {
        Some(PortableRecord::PackageHeader { format, version })
            if format == "agentSchedule" && version == 1 => {}
        _ => return Err(PackageError::InvalidRecord),
    }
    let instruction = match records.next() {
        Some(PortableRecord::InstructionDocument {
            instruction_id,
            text,
            source_revision_id,
        }) if !source_revision_id.is_empty() => crate::PortableInstruction {
            instruction_id,
            text,
            source_revision_id,
        },
        _ => return Err(PackageError::InvalidRecord),
    };
    let (schedule_id, source_change_id, mut definition) = match records.next() {
        Some(PortableRecord::ScheduleDefinition {
            schedule_id,
            source_change_id,
            definition,
        }) if !source_change_id.is_empty() => (schedule_id, source_change_id, definition),
        _ => return Err(PackageError::InvalidRecord),
    };
    if definition.instruction_id != instruction.instruction_id
        || !matches!(
            definition.destination,
            crate::ExecutionDestination::Unprepared
        )
    {
        return Err(PackageError::InvalidDefinition);
    }
    definition.enabled = false;
    let continuity = match records.next() {
        None => None,
        Some(PortableRecord::ContinuitySummary {
            text,
            source_run_id,
            source_target,
        }) if !source_run_id.is_empty() => Some(crate::PortableContinuity {
            text,
            source_run_id,
            source_target,
        }),
        _ => return Err(PackageError::InvalidRecord),
    };
    if records.next().is_some() {
        return Err(PackageError::InvalidRecord);
    }
    Ok(PortableSchedulePackage {
        schedule_id,
        source_change_id,
        instruction,
        definition,
        continuity,
    })
}
