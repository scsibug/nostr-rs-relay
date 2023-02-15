use tonic::{transport::Server, Request, Response, Status};

use nauthz_grpc::authorization_server::{Authorization, AuthorizationServer};
use nauthz_grpc::{Decision, EventReply, EventRequest};

pub mod nauthz_grpc {
    tonic::include_proto!("nauthz");
}

#[derive(Default)]
pub struct EventAuthz {
    allowed_kinds: Vec<u64>,
}

#[tonic::async_trait]
impl Authorization for EventAuthz {
    async fn event_admit(
        &self,
        request: Request<EventRequest>,
    ) -> Result<Response<EventReply>, Status> {
        let reply;
        let req = request.into_inner();
        let event = req.event.unwrap();
        let content_prefix: String = event.content.chars().take(40).collect();
        println!("recvd event, [kind={}, origin={:?}, nip05_domain={:?}, tag_count={}, content_sample={:?}]",
                 event.kind, req.origin, req.nip05.map(|x| x.domain), event.tags.len(), content_prefix);
        // Permit any event with a whitelisted kind
        if self.allowed_kinds.contains(&event.kind) {
            println!("This looks fine! (kind={})", event.kind);
            reply = nauthz_grpc::EventReply {
                decision: Decision::Permit as i32,
                message: None,
            };
        } else {
            println!("Blocked! (kind={})", event.kind);
            reply = nauthz_grpc::EventReply {
                decision: Decision::Deny as i32,
                message: Some(format!("kind {} not permitted", event.kind)),
            };
        }
        Ok(Response::new(reply))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "[::1]:50051".parse().unwrap();

    // A simple authorization engine that allows kinds 0-3
    let checker = EventAuthz {
        allowed_kinds: vec![0, 1, 2, 3],
    };
    println!("EventAuthz Server listening on {}", addr);
    // Start serving
    Server::builder()
        .add_service(AuthorizationServer::new(checker))
        .serve(addr)
        .await?;
    Ok(())
}
