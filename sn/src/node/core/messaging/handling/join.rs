// Copyright 2022 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use crate::elder_count;
use crate::messaging::system::{
    JoinAsRelocatedRequest, JoinAsRelocatedResponse, JoinRejectionReason, JoinRequest,
    JoinResponse, MembershipState, SystemMsg,
};
use crate::node::{
    api::cmds::Cmd,
    core::{relocation::RelocateDetailsUtils, Node},
    Error, Result, SectionAuthUtils, FIRST_SECTION_MAX_AGE, MIN_ADULT_AGE,
};
use crate::types::{log_markers::LogMarker, Peer};

use bls::PublicKey as BlsPublicKey;
use std::vec;

const FIRST_SECTION_MIN_ELDER_AGE: u8 = 90;

// Message handling
impl Node {
    pub(crate) async fn handle_join_request(
        &self,
        peer: Peer,
        join_request: JoinRequest,
    ) -> Result<Vec<Cmd>> {
        debug!("Received {:?} from {}", join_request, peer);

        // To avoid existing elders hanving different view of section members,
        // JoinRequest for resource_proofing and aggregated NodeState
        // shall be handled immediately without any semaphore or age check.

        // If the joining node has aggregated shares from enough elders,
        // verify the produced auth and accept it into the network.
        if let Some(response) = join_request.aggregated {
            if response.verify(&self.section_chain().await) {
                info!("Handling Online agreement of {:?}", peer);
                return Ok(vec![Cmd::HandleNewNodeOnline(response)]);
            }
        }

        // Require resource signed if joining as a new node.
        if let Some(response) = join_request.resource_proof_response {
            if !self
                .validate_resource_proof_response(&peer.name(), response)
                .await
            {
                debug!(
                    "Ignoring JoinRequest from {} - invalid resource signed response",
                    peer
                );
                return Ok(vec![]);
            } else {
                return Ok(vec![Cmd::SendAcceptedOnlineShare {
                    peer,
                    previous_name: None,
                }]);
            }
        }

        let _permit = self
            .current_joins_semaphore
            .acquire()
            .await
            .map_err(|_| Error::PermitAcquisitionFailed)?;

        let our_section_key = self.network_knowledge.section_key().await;
        let section_key_matches = join_request.section_key == our_section_key;

        // Ignore `JoinRequest` if we are not elder, unless the join request
        // is outdated in which case we'll reply with `JoinResponse::Retry`
        // with the up-to-date info.
        if self.is_not_elder().await && section_key_matches {
            // Note: We don't bounce this message because the current bounce-resend
            // mechanism wouldn't preserve the original SocketAddr which is needed for
            // properly handling this message.
            // This is OK because in the worst case the join request just timeouts and the
            // joining node sends it again.
            return Ok(vec![]);
        }

        // Check if we already have this peer in our section
        if self.network_knowledge.is_section_member(&peer.name()).await {
            debug!(
                "Ignoring JoinRequest from {} - already member of our section.",
                peer
            );
            return Ok(vec![]);
        }

        let our_prefix = self.network_knowledge.prefix().await;
        if !our_prefix.matches(&peer.name()) {
            debug!(
                "Redirecting JoinRequest from {} - name doesn't match our prefix {:?}.",
                peer, our_prefix
            );

            let retry_sap = self.matching_section(&peer.name()).await?;

            let node_msg =
                SystemMsg::JoinResponse(Box::new(JoinResponse::Redirect(retry_sap.to_msg())));

            trace!("Sending {:?} to {}", node_msg, peer);
            trace!("{}", LogMarker::SendJoinRedirected);
            return Ok(vec![
                self.send_direct_msg(peer, node_msg, our_section_key)
                    .await?,
            ]);
        }

        if !*self.joins_allowed.read().await {
            debug!(
                "Rejecting JoinRequest from {} - joins currently not allowed.",
                peer,
            );
            let node_msg = SystemMsg::JoinResponse(Box::new(JoinResponse::Rejected(
                JoinRejectionReason::JoinsDisallowed,
            )));

            trace!("{}", LogMarker::SendJoinsDisallowed);

            trace!("Sending {:?} to {}", node_msg, peer);
            return Ok(vec![
                self.send_direct_msg(peer, node_msg, our_section_key)
                    .await?,
            ]);
        }

        let (is_age_invalid, expected_age) = self.verify_joining_node_age(&peer).await;

        trace!(
            "our_prefix {:?} expected_age {:?} is_age_invalid {:?}",
            our_prefix,
            expected_age,
            is_age_invalid
        );

        if !section_key_matches || is_age_invalid {
            if !section_key_matches {
                trace!("{}", LogMarker::SendJoinRetryNotCorrectKey);
                trace!(
                    "JoinRequest from {} doesn't have our latest section_key {:?}, presented {:?}.",
                    peer,
                    our_section_key,
                    join_request.section_key
                );
            } else {
                trace!("{}", LogMarker::SendJoinRetryAgeIssue);
                trace!(
                    "JoinRequest from {} (with age {}) doesn't have the expected: {}",
                    peer,
                    peer.age(),
                    expected_age,
                );
            }

            let proof_chain = self.network_knowledge.section_chain().await;
            let signed_sap = self
                .network_knowledge
                .section_signed_authority_provider()
                .await;

            let node_msg = SystemMsg::JoinResponse(Box::new(JoinResponse::Retry {
                section_auth: signed_sap.value.to_msg(),
                section_signed: signed_sap.sig,
                proof_chain,
                expected_age,
            }));

            trace!("Sending {:?} to {}", node_msg, peer);
            return Ok(vec![
                self.send_direct_msg(peer, node_msg, our_section_key)
                    .await?,
            ]);
        }

        // Do reachability check only for the initial join request
        let cmd = if self.comm.is_reachable(&peer.addr()).await.is_err() {
            let node_msg = SystemMsg::JoinResponse(Box::new(JoinResponse::Rejected(
                JoinRejectionReason::NodeNotReachable(peer.addr()),
            )));

            trace!("{}", LogMarker::SendJoinRejected);

            trace!("Sending {:?} to {}", node_msg, peer);
            self.send_direct_msg(peer, node_msg, our_section_key)
                .await?
        } else {
            // It's reachable, let's then send the proof challenge
            self.send_resource_proof_challenge(peer).await?
        };

        Ok(vec![cmd])
    }

    pub(crate) async fn verify_joining_node_age(&self, peer: &Peer) -> (bool, u8) {
        // During the first section, nodes shall use ranged age to avoid too many nodes getting
        // relocated at the same time. After the first section splits, nodes shall only
        // start with an age of MIN_ADULT_AGE
        let current_section_size = self.network_knowledge.section_size().await;
        let our_prefix = self.network_knowledge.prefix().await;

        // Prefix will be empty for first section
        if our_prefix.is_empty() {
            let elders = self.network_knowledge.elders().await;
            // Forces the joining node to be younger than the youngest elder in genesis section
            // avoiding unnecessary churn.

            // Check if `elder_count()` Elders are already present
            if elders.len() == elder_count() {
                // Check if the joining node is younger than the youngest elder and older than
                // MIN_ADULT_AGE in the first section, to avoid unnecessary churn during genesis.
                let expected_age = FIRST_SECTION_MIN_ELDER_AGE - current_section_size as u8 * 2;
                let is_age_invalid = peer.age() <= MIN_ADULT_AGE || peer.age() > expected_age;
                (is_age_invalid, expected_age)
            } else {
                // Since enough elders haven't joined the first section calculate a value
                // within the range [FIRST_SECTION_MIN_ELDER_AGE, FIRST_SECTION_MAX_AGE].
                let expected_age = FIRST_SECTION_MAX_AGE - current_section_size as u8 * 2;
                // TODO: avoid looping by ensure can only update to lower non-FIRST_SECTION_MIN_ELDER_AGE age
                let is_age_invalid = peer.age() != expected_age;
                (is_age_invalid, expected_age)
            }
        } else {
            // Age should be MIN_ADULT_AGE for joining nodes after genesis section.
            let is_age_invalid = peer.age() != MIN_ADULT_AGE;
            (is_age_invalid, MIN_ADULT_AGE)
        }
    }

    pub(crate) async fn handle_join_as_relocated_request(
        &self,
        peer: Peer,
        join_request: JoinAsRelocatedRequest,
        known_keys: Vec<BlsPublicKey>,
    ) -> Result<Vec<Cmd>> {
        let _permit = self
            .current_joins_semaphore
            .acquire()
            .await
            .map_err(|_| Error::PermitAcquisitionFailed)?;

        debug!(
            "Received JoinAsRelocatedRequest {:?} from {}",
            join_request, peer
        );

        let our_prefix = self.network_knowledge.prefix().await;
        if !our_prefix.matches(&peer.name())
            || join_request.section_key != self.network_knowledge.section_key().await
        {
            debug!(
                "JoinAsRelocatedRequest from {} - name doesn't match our prefix {:?}.",
                peer, our_prefix
            );

            let node_msg =
                SystemMsg::JoinAsRelocatedResponse(Box::new(JoinAsRelocatedResponse::Retry(
                    self.network_knowledge.authority_provider().await.to_msg(),
                )));

            trace!("{} b", LogMarker::SendJoinAsRelocatedResponse);

            trace!("Sending {:?} to {}", node_msg, peer);
            return Ok(vec![
                self.send_direct_msg(peer, node_msg, self.network_knowledge.section_key().await)
                    .await?,
            ]);
        }

        if self.network_knowledge.is_section_member(&peer.name()).await {
            debug!(
                "Ignoring JoinAsRelocatedRequest from {} - already member of our section.",
                peer
            );
            return Ok(vec![]);
        }

        let relocate_details = if let MembershipState::Relocated(ref details) =
            join_request.relocate_proof.value.state
        {
            // Check for signatures and trust of the relocate_proof
            let is_valid_sig = join_request.relocate_proof.self_verify();
            let is_key_unknown = !known_keys
                .iter()
                .any(|key| *key == join_request.relocate_proof.sig.public_key);

            if !is_valid_sig || is_key_unknown {
                debug!(
                    "Ignoring JoinAsRelocatedRequest from {} - invalid signature or untrusted src.",
                    peer
                );
                return Ok(vec![]);
            }

            details
        } else {
            debug!(
                "Ignoring JoinAsRelocatedRequest from {} with invalid relocate proof state: {:?}",
                peer, join_request.relocate_proof.value.state
            );
            return Ok(vec![]);
        };

        if !relocate_details.verify_identity(&peer.name(), &join_request.signature_over_new_name) {
            debug!(
                "Ignoring JoinAsRelocatedRequest from {} - invalid node name signature.",
                peer
            );
            return Ok(vec![]);
        }

        let prefix = self.network_knowledge.prefix().await;
        if !prefix.matches(&relocate_details.dst) {
            debug!(
                "Ignoring JoinAsRelocatedRequest from {} - destination {} doesn't match \
                         our prefix {:?}.",
                peer, relocate_details.dst, prefix
            );
            return Ok(vec![]);
        }

        // Requires the node name matches the age.
        let age = relocate_details.age;
        if age != peer.age() {
            debug!(
                "Ignoring JoinAsRelocatedRequest from {} - relocation age ({}) doesn't match peer's age ({}).",
                peer, age, peer.age(),
            );
            return Ok(vec![]);
        }

        if self
            .network_knowledge
            .is_relocated_to_our_section(&relocate_details.previous_name)
            .await
        {
            debug!(
                "Ignoring JoinAsRelocatedRequest from {} - original node {:?} already relocated to us.",
                peer, relocate_details.previous_name
            );
            return Ok(vec![]);
        }

        // Finally do reachability check
        if self.comm.is_reachable(&peer.addr()).await.is_err() {
            let node_msg = SystemMsg::JoinAsRelocatedResponse(Box::new(
                JoinAsRelocatedResponse::NodeNotReachable(peer.addr()),
            ));
            trace!("{}", LogMarker::SendJoinAsRelocatedResponse);

            trace!("Sending {:?} to {}", node_msg, peer);
            return Ok(vec![
                self.send_direct_msg(peer, node_msg, self.network_knowledge.section_key().await)
                    .await?,
            ]);
        };

        Ok(vec![Cmd::SendAcceptedOnlineShare {
            peer,
            previous_name: Some(relocate_details.previous_name),
        }])
    }
}
