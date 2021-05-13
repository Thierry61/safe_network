// Copyright 2021 MaidSafe.net limited.
//
// This SAFE Network Software is licensed to you under The General Public License (GPL), version 3.
// Unless required by applicable law or agreed to in writing, the SAFE Network Software distributed
// under the GPL Licence is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied. Please review the Licences for the specific language governing
// permissions and limitations relating to use of the SAFE Network Software.

use super::{Mapping, MsgContext};
use crate::{
    error::convert_to_error_message,
    node_ops::{MsgType, NodeDuty, OutgoingMsg},
    Error,
};
use sn_messaging::{
    client::{ClientMsg, Cmd, ProcessMsg, ProcessingError, Query, TransferCmd, TransferQuery},
    Aggregation, EndUser, MessageId, Msg, SrcLocation,
};

pub fn map_client_msg(msg: ProcessMsg, origin: EndUser) -> Mapping {
    match msg.to_owned() {
        ProcessMsg::Query {
            query: Query::Data(query),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::ProcessRead { query, id, origin },
            ctx: Some(super::MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        ProcessMsg::Cmd {
            cmd: Cmd::Data { .. },
            ..
        } => Mapping::Ok {
            op: NodeDuty::ProcessDataPayment {
                msg: msg.clone(),
                origin,
            },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        ProcessMsg::Cmd {
            cmd: Cmd::Transfer(TransferCmd::ValidateTransfer(signed_transfer)),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::ValidateClientTransfer {
                signed_transfer,
                origin: SrcLocation::EndUser(origin),
                msg_id: id,
            },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        // TODO: Map more transfer cmds
        ProcessMsg::Cmd {
            cmd: Cmd::Transfer(TransferCmd::SimulatePayout(transfer)),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::SimulatePayout {
                transfer,
                origin: SrcLocation::EndUser(origin),
                msg_id: id,
            },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        ProcessMsg::Cmd {
            cmd: Cmd::Transfer(TransferCmd::RegisterTransfer(proof)),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::RegisterTransfer { proof, msg_id: id },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        // TODO: Map more transfer queries
        ProcessMsg::Query {
            query: Query::Transfer(TransferQuery::GetHistory { at, since_version }),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::GetTransfersHistory {
                at,
                since_version,
                origin: SrcLocation::EndUser(origin),
                msg_id: id,
            },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        ProcessMsg::Query {
            query: Query::Transfer(TransferQuery::GetBalance(at)),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::GetBalance {
                at,
                origin: SrcLocation::EndUser(origin),
                msg_id: id,
            },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        ProcessMsg::Query {
            query: Query::Transfer(TransferQuery::GetStoreCost { bytes, .. }),
            id,
            ..
        } => Mapping::Ok {
            op: NodeDuty::GetStoreCost {
                bytes,
                origin: SrcLocation::EndUser(origin),
                msg_id: id,
            },
            ctx: Some(MsgContext::Msg {
                msg: Msg::Client(ClientMsg::Process(msg)),
                src: SrcLocation::EndUser(origin),
            }),
        },
        _ => {
            let msg_id = msg.id();
            let error_data = convert_to_error_message(Error::InvalidMessage(
                msg.id(),
                format!("Unknown user msg: {:?}", msg),
            ));
            let src = SrcLocation::EndUser(origin);

            Mapping::Ok {
                ctx: Some(MsgContext::Msg {
                    msg: Msg::Client(ClientMsg::Process(msg.clone())),
                    src,
                }),
                op: NodeDuty::Send(OutgoingMsg {
                    msg: MsgType::Client(ClientMsg::ProcessingError(ProcessingError::new(
                        Some(error_data),
                        Some(msg),
                        MessageId::in_response_to(&msg_id),
                    ))),
                    section_source: false, // strictly this is not correct, but we don't expect responses to an error..
                    dst: src.to_dst(),
                    aggregation: Aggregation::None,
                }),
            }
        }
    }
}
