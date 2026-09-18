use catplay_csm::{
    decoder::{AsCsmPacket, CsmByteArray, CsmPacketUtil},
    msg::{
        AccessoryWiFiConfigurationInformation, AuthenticationCertificate, AuthenticationFailed, AuthenticationResponse,
        AuthenticationSucceeded, IdentificationAccepted, IdentificationInformation, IdentificationRejected,
        RequestAccessoryWiFiConfigurationInformation, RequestAuthenticationCertificate, RequestAuthenticationChallengeResponse,
        StartIdentification,
    },
};
use log::debug;

use crate::{CsmClientHandleRef, CsmSessionResult};

pub struct ClientSessionHelper {}

impl ClientSessionHelper {
    #[cfg(feature = "mfi")]
    pub async fn handle_auth(
        mfi: Option<catplay_mfi::MfiDeficeRef>,
        packet: &dyn AsCsmPacket,
        client: CsmClientHandleRef,
    ) -> CsmSessionResult<bool> {
        if RequestAuthenticationCertificate::cast(packet).is_some() {
            return match mfi {
                None => Err("RequestAuthenticationCertificate received without MFI device registered".into()),
                Some(mfi) => {
                    debug!("Reading MFI cert");
                    let cert = mfi.read_certificate()?;
                    debug!("Responding to RequestAuthenticationCertificate");

                    client.send(&AuthenticationCertificate {
                        authentication_certificate: cert.into(),
                    })?;
                    return Ok(true);
                }
            };
        }

        if let Some(racr) = RequestAuthenticationChallengeResponse::cast(packet) {
            use catplay_util::spawn_blocking;

            let Some(mfi) = mfi else {
                return Err("RequestAuthenticationChallengeResponse received without MFI device registered".into());
            };

            debug!("Signing MFI challenge ({} bytes)", racr.authentication_challenge.data.len());
            let challenge = racr.authentication_challenge.data.clone();
            let mfi = mfi.clone();

            let response: CsmByteArray = spawn_blocking(move || mfi.generate_challenge_response(&challenge[..]))
                .await
                .map_err(|error| format!("MFI signing worker failed: {error}"))??
                .into();
            debug!(
                "Responding to RequestAuthenticationChallengeResponse ({} bytes)",
                response.data.len()
            );

            client.send(&AuthenticationResponse {
                authentication_response: response,
            })?;
            return Ok(true);
        }

        if AuthenticationSucceeded::cast(packet).is_some() {
            return Ok(true);
        }

        if AuthenticationFailed::cast(packet).is_some() {
            debug!("Received AuthenticationFailed :( MFI problem?");
            return Err("Received AuthenticationFailed from the iPhone: {}".into());
        }

        Ok(false)
    }

    pub async fn handle_id(
        packet: &dyn AsCsmPacket,
        client: CsmClientHandleRef,
        accepted: &mut bool,
        id: &IdentificationInformation,
    ) -> CsmSessionResult<bool> {
        if StartIdentification::cast(packet).is_some() {
            debug!("Responding to StartIdentification");
            debug!("Our ID: {:?}", id);
            client.send(id)?;
            return Ok(true);
        }

        if IdentificationAccepted::cast(packet).is_some() {
            debug!("Identification accepted!");
            *accepted = true;
            return Ok(true);
        }

        if let Some(ir) = IdentificationRejected::cast(packet) {
            debug!("Identification rejected :( flags: {:?}", ir);
            return Err(format!("Host rejected identification information: {}", ir.reason()).into());
        }

        Ok(false)
    }

    pub async fn handle_wifi(
        packet: &dyn AsCsmPacket,
        client: CsmClientHandleRef,
        redirected: &mut bool,
        id: &AccessoryWiFiConfigurationInformation,
    ) -> CsmSessionResult<bool> {
        if RequestAccessoryWiFiConfigurationInformation::cast(packet).is_some() {
            debug!("Responding to RequestAccessoryWiFiConfigurationInformation!");
            *redirected = true;
            client.send(id)?;
            return Ok(true);
        }

        Ok(false)
    }
}
