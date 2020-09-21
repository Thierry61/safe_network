// Copyright 2020 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use crate::{
    node::node_ops::{NodeDuty, NodeOperation, RewardDuty},
    Network,
};
use sn_data_types::PublicKey;

// Called after connected to network.
// Configuration calls made
// as part of a node initialising.
// 1. Add own node id to rewards
// 2. Add own wallet to rewards
// 3. Register wallet at Elders

pub struct Startup {
    reward_key: PublicKey,
    network_api: Network,
}

impl Startup {
    pub fn new(reward_key: PublicKey, network_api: Network) -> Self {
        Self {
            reward_key,
            network_api,
        }
    }

    pub async fn init(&mut self) -> Option<NodeOperation> {
        let node_id = self.network_api.name().await;
        // 1. Add own node id to rewards
        let first: NodeOperation = RewardDuty::AddNewNode(node_id).into();
        // 2. Add own wallet to rewards
        let second = RewardDuty::SetNodeWallet {
            node_id,
            wallet_id: self.reward_key,
        }
        .into();
        // 3. Register wallet at Elders
        let third = NodeDuty::RegisterWallet(self.reward_key).into();
        Some(vec![first, second, third].into())
    }
}
