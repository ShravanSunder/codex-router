//! Accept typed Approver identities while keeping existing SessionRef actors readable.

use crate::SessionRef;
use message_board::Identity;
use serde::Deserialize;

pub(crate) fn deserialize_interaction_actor<'de, D>(deserializer: D) -> Result<Identity, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ActorWire {
        Typed(Identity),
        Legacy(SessionRef),
    }
    match ActorWire::deserialize(deserializer)? {
        ActorWire::Typed(actor) => Ok(actor),
        ActorWire::Legacy(session) => {
            let board_session = message_board::SessionRef {
                endpoint: message_board::SessionEndpointRef {
                    service_id: message_board::ServiceId::try_from(String::from(
                        session.endpoint.service_id,
                    ))
                    .map_err(serde::de::Error::custom)?,
                    endpoint_id: message_board::EndpointId::try_from(String::from(
                        session.endpoint.endpoint_id,
                    ))
                    .map_err(serde::de::Error::custom)?,
                },
                session_id: message_board::SessionId::try_from(String::from(session.session_id))
                    .map_err(serde::de::Error::custom)?,
            };
            Ok(Identity::Session {
                session: board_session,
            })
        }
    }
}
