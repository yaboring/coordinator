use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child as TokioChild, Command as TokioCommand};
use tokio::sync::Mutex;
use tokio::time::Duration;
use uuid::Uuid;
use warp::Filter;

#[derive(Serialize, Deserialize, Debug, Clone)]
enum MessageType {
    Request,    // Expects a response
    Response,   // No response expected
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct EntityMessage {
    id: String,
    from: String,
    to: String,
    content: String,
    message_type: MessageType,
    timestamp: u64,
}

#[derive(Debug)]
struct ManagedEntity {
    entity_id: String,
    entity_name: Option<String>,
    entity_type: Option<String>,
    process: Option<TokioChild>,
    tcp_port: u16,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct CoordinatorInfo {
    coordinator_id: String,
    host: String,
    port: u16,
}

struct NetworkedCoordinator {
    coordinator_id: String,
    host: String,
    port: u16,
    entities: Arc<Mutex<HashMap<String, ManagedEntity>>>,
    entity_binary_path: String,
    // Track where remote entities live
    entity_directory: Arc<Mutex<HashMap<String, CoordinatorInfo>>>,
    // Known peer coordinators
    peer_coordinators: Arc<Mutex<Vec<CoordinatorInfo>>>,
    http_client: reqwest::Client,
}

impl NetworkedCoordinator {
    fn new(coordinator_id: String, host: String, port: u16) -> Self {
        Self {
            coordinator_id,
            host,
            port,
            entities: Arc::new(Mutex::new(HashMap::new())),
            entity_binary_path: "../entity-core/target/release/entity-core".to_string(),
            entity_directory: Arc::new(Mutex::new(HashMap::new())),
            peer_coordinators: Arc::new(Mutex::new(Vec::new())),
            http_client: reqwest::Client::new(),
        }
    }

    async fn add_peer_coordinator(&self, peer: CoordinatorInfo) {
        let mut peers = self.peer_coordinators.lock().await;
        println!("🤝 Added peer coordinator: {}:{}", peer.host, peer.port);
        peers.push(peer);
    }

    // NOTE: no &mut self needed — internal state is behind mutexes.
    async fn spawn_entity(&self) -> Result<String, Box<dyn std::error::Error>> {
        let entity_id = Uuid::new_v4().to_string();
        let coordinator_url = format!("http://{}:{}", self.host, self.port);

        println!("🚀 Spawning entity: {} (will self-register)", entity_id);

        // Spawn the entity process with entity ID and coordinator URL as arguments
        let mut child = TokioCommand::new(&self.entity_binary_path)
            .arg(&entity_id)     // Pass the entity ID as first argument
            .arg(&coordinator_url)  // Pass the coordinator URL as second argument
            .stderr(Stdio::piped())
            .spawn()?;

        // Only handle stderr for entity debug output
        let stderr = child.stderr.take().unwrap();
        let coordinator_id = self.coordinator_id.clone();
        let entity_id_for_stderr = entity_id.clone();
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                println!("[{}/{}] {}", coordinator_id, entity_id_for_stderr, line);
            }
        });

        // Store only the process handle - entity will fully register itself when ready
        {
            let mut entities = self.entities.lock().await;
            entities.insert(
                entity_id.clone(),
                ManagedEntity {
                    entity_id: entity_id.clone(),
                    entity_name: None,
                    entity_type: None,
                    process: Some(child),
                    tcp_port: 0, // Entity not yet registered
                },
            );
        }

        println!("🚀 Entity {} process started, waiting for self-registration", entity_id);

        Ok(entity_id)
    }

    async fn is_entity_local(&self, entity_id: &str) -> bool {
        let entities = self.entities.lock().await;
        entities.get(entity_id)
            .map(|entity| entity.tcp_port > 0) // Only consider entities with valid TCP port as available
            .unwrap_or(false)
    }

    async fn send_tcp_message_to_local_entity(
        &self,
        target_entity_id: &str,
        message: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        use tokio::net::TcpStream;
        use tokio::io::{AsyncWriteExt, AsyncBufReadExt, BufReader};
        
        let tcp_port = {
            let entities = self.entities.lock().await;
            entities
                .get(target_entity_id)
                .map(|entity| entity.tcp_port)
                .ok_or_else(|| format!("entity {} not found locally", target_entity_id))?
        };

        // Connect to entity's TCP port
        let mut stream = TcpStream::connect(format!("127.0.0.1:{}", tcp_port)).await?;
        
        // Send message
        stream.write_all(message.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        
        // Read response
        let (reader, _writer) = stream.split();
        let mut reader = BufReader::new(reader);
        let mut response = String::new();
        reader.read_line(&mut response).await?;
        
        Ok(response.trim().to_string())
    }





    async fn get_entity_list(&self) -> Vec<String> {
        let entities = self.entities.lock().await;
        entities.iter()
            .filter(|(_, entity)| entity.tcp_port > 0) // Only return fully registered entities
            .map(|(id, _)| id.clone())
            .collect()
    }

    // HTTP proxy method: send request to local entity and wait for response
    async fn ask_local_entity(
        &self,
        target_entity_id: &str,
        message_content: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Send TCP message to entity and get response
        self.send_tcp_message_to_local_entity(target_entity_id, message_content).await
    }

    // HTTP proxy method: send request to remote entity via its coordinator
    async fn ask_remote_entity(
        &self,
        target_entity_id: &str,
        message_content: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        // Look up which coordinator has this entity
        let target_coordinator = {
            let directory = self.entity_directory.lock().await;
            directory.get(target_entity_id).cloned()
        };

        if let Some(coordinator_info) = target_coordinator {
            // Forward HTTP request to target coordinator
            let url = format!(
                "http://{}:{}/ask_entity/{}",
                coordinator_info.host, coordinator_info.port, target_entity_id
            );
            
            let response = self.http_client
                .post(&url)
                .header("Content-Type", "text/plain")
                .body(message_content.to_string())
                .send()
                .await?;

            if response.status().is_success() {
                let response_text = response.text().await?;
                Ok(response_text)
            } else {
                Err(format!("Remote coordinator request failed: {}", response.status()).into())
            }
        } else {
            Err(format!("Entity {} not found in directory", target_entity_id).into())
        }
    }

    async fn share_entity_directory(
        &self,
        peer_coordinator: &CoordinatorInfo,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Snapshot our local entries first, then send without holding the lock
        let local_entries: Vec<(String, CoordinatorInfo)> = {
            let directory = self.entity_directory.lock().await;
            directory
                .iter()
                .filter(|(_, info)| info.coordinator_id == self.coordinator_id)
                .map(|(id, info)| (id.clone(), info.clone()))
                .collect()
        };

        for (entity_id, coordinator_info) in local_entries {
            let url = format!("http://{}:{}/register_entity", peer_coordinator.host, peer_coordinator.port);

            #[derive(Serialize)]
            struct EntityRegistration {
                entity_id: String,
                coordinator: CoordinatorInfo,
            }

            let registration = EntityRegistration {
                entity_id,
                coordinator: coordinator_info,
            };

            if let Err(e) = self.http_client.post(&url).json(&registration).send().await {
                eprintln!("Failed to share entity directory with peer: {}", e);
            }
        }

        Ok(())
    }
}

async fn create_http_server(
    port: u16,
    coordinator: Arc<NetworkedCoordinator>,
) -> Result<(), Box<dyn std::error::Error>> {
    // Clone for the HTTP handlers
    let coordinator_for_register = coordinator.clone();
    let coordinator_for_ask = coordinator.clone();

    // POST /ask_entity/{target_id} - HTTP proxy for entity-to-entity communication
    let ask_entity_route = warp::path!("ask_entity" / String)
        .and(warp::post())
        .and(warp::body::bytes())
        .and_then(move |target_entity_id: String, body: warp::hyper::body::Bytes| {
            let coordinator = coordinator_for_ask.clone();
            async move {
                let message_content = String::from_utf8_lossy(&body).to_string();
                
                eprintln!("🌐 HTTP proxy request to entity {}: {}", target_entity_id, message_content);
                
                if coordinator.is_entity_local(&target_entity_id).await {
                    // Entity is local - send request and wait for response
                    match coordinator.ask_local_entity(&target_entity_id, &message_content).await {
                        Ok(response) => {
                            eprintln!("✅ HTTP proxy response: {}", response);
                            Ok::<String, warp::Rejection>(response)
                        }
                        Err(e) => {
                            eprintln!("❌ HTTP proxy failed: {}", e);
                            Err(warp::reject())
                        }
                    }
                } else {
                    // Entity is remote - forward to target coordinator
                    match coordinator.ask_remote_entity(&target_entity_id, &message_content).await {
                        Ok(response) => {
                            eprintln!("✅ HTTP proxy remote response: {}", response);
                            Ok::<String, warp::Rejection>(response)
                        }
                        Err(e) => {
                            eprintln!("❌ HTTP proxy remote failed: {}", e);
                            Err(warp::reject())
                        }
                    }
                }
            }
        });



    // POST /register_entity - Handle both self-registration and cross-coordinator registration
    let register_route = warp::path("register_entity")
        .and(warp::post())
        .and(warp::body::json())
        .and_then(move |registration: serde_json::Value| {
            let coordinator = coordinator_for_register.clone();
            async move {
                if let Some(entity_id) = registration.get("entity_id").and_then(|v| v.as_str()) {
                    // Check if this is self-registration (entity on this coordinator)
                    if let Some(tcp_port) = registration.get("tcp_port").and_then(|v| v.as_u64()) {
                        // Self-registration: update local entity with TCP port and add to entity directory
                        let mut entities = coordinator.entities.lock().await;
                        if let Some(entity) = entities.get_mut(entity_id) {
                            entity.tcp_port = tcp_port as u16;
                            if let Some(name) = registration.get("entity_name").and_then(|v| v.as_str()) {
                                entity.entity_name = Some(name.to_string());
                            }
                            if let Some(entity_type) = registration.get("entity_type").and_then(|v| v.as_str()) {
                                entity.entity_type = Some(entity_type.to_string());
                            }
                            println!("✅ Entity {} self-registered on TCP port {}", entity_id, tcp_port);
                            
                            // Now that entity is fully registered, add it to the entity directory
                            drop(entities); // Release entities lock before acquiring directory lock
                            {
                                let mut directory = coordinator.entity_directory.lock().await;
                                directory.insert(
                                    entity_id.to_string(),
                                    CoordinatorInfo {
                                        coordinator_id: coordinator.coordinator_id.clone(),
                                        host: coordinator.host.clone(),
                                        port: coordinator.port,
                                    },
                                );
                                println!("📝 Entity {} added to entity directory", entity_id);
                            }
                        }
                    } else if let Some(coordinator_info) = serde_json::from_value::<CoordinatorInfo>(
                        registration.get("coordinator").cloned().unwrap_or(serde_json::Value::Null),
                    ).ok() {
                        // Cross-coordinator registration
                        let mut directory = coordinator.entity_directory.lock().await;
                        directory.insert(entity_id.to_string(), coordinator_info);
                        println!("📝 Registered remote entity: {}", entity_id);
                    }
                }

                Ok::<&str, warp::Rejection>("Entity registered")
            }
        });


    let entities_route = warp::path("entities")
        .and(warp::get())
        .and_then({
            let coord = coordinator.clone();
            move || {
                let coord = coord.clone();
                async move {
                    let ids = coord.get_entity_list().await;
                    Ok::<warp::reply::Json, warp::Rejection>(warp::reply::json(&ids))
                }
            }
        });

    // POST /spawn_entity - Manual entity spawning with optional overrides
    let spawn_entity_route = warp::path("spawn_entity")
        .and(warp::post())
        .and(warp::body::json())
        .and_then({
            let coord = coordinator.clone();
            move |spawn_request: serde_json::Value| {
                let coord = coord.clone();
                async move {
                    println!("🚀 Manual entity spawn requested: {:?}", spawn_request);
                    
                    match coord.spawn_entity().await {
                        Ok(entity_id) => {
                            println!("✅ Manually spawned entity: {}", entity_id);
                            
                            // TODO: Handle optional overrides from spawn_request
                            // For now, just use the default generated values
                            // In the future, could override entity_name, entity_type, position, etc.
                            
                            Ok::<warp::reply::Json, warp::Rejection>(warp::reply::json(&serde_json::json!({
                                "success": true,
                                "entity_id": entity_id,
                                "message": "Entity spawned successfully"
                            })))
                        }
                        Err(e) => {
                            eprintln!("❌ Failed to spawn entity: {}", e);
                            Ok::<warp::reply::Json, warp::Rejection>(warp::reply::json(&serde_json::json!({
                                "success": false,
                                "error": format!("Failed to spawn entity: {}", e)
                            })))
                        }
                    }
                }
            }
        });

    let routes = ask_entity_route.or(register_route).or(entities_route).or(spawn_entity_route);

    println!("🌐 HTTP server starting on port {}", port);
    warp::serve(routes).run(([127, 0, 0, 1], port)).await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse command line args
    let args: Vec<String> = std::env::args().collect();
    
    if args.len() < 2 {
        eprintln!("Usage: coordinator <port> [peer_port] [--entities N]");
        eprintln!("  port: HTTP server port (e.g., 9000)");
        eprintln!("  peer_port: Optional peer coordinator port");  
        eprintln!("  --entities N: Auto-spawn N entities on startup");
        return Ok(());
    }
    
    let port: u16 = args[1].parse().unwrap_or(9000);
    let mut peer_port: Option<u16> = None;
    let mut auto_spawn_count: Option<usize> = None;
    
    // Parse remaining args
    let mut i = 2;
    while i < args.len() {
        match args[i].as_str() {
            "--entities" => {
                if i + 1 < args.len() {
                    auto_spawn_count = args[i + 1].parse().ok();
                    i += 2;
                } else {
                    eprintln!("Error: --entities requires a number");
                    return Ok(());
                }
            }
            arg if arg.parse::<u16>().is_ok() && peer_port.is_none() => {
                peer_port = arg.parse().ok();
                i += 1;
            }
            _ => {
                eprintln!("Unknown argument: {}", args[i]);
                i += 1;
            }
        }
    }

    let coordinator_id = format!("coordinator-{}", port);
    println!("🏗️  Starting Coordinator {}...", coordinator_id);

    let coordinator = NetworkedCoordinator::new(coordinator_id, "127.0.0.1".to_string(), port);
    let coordinator_arc = Arc::new(coordinator);

    // If peer port specified, add it as a peer
    if let Some(peer_port) = peer_port {
        coordinator_arc
            .add_peer_coordinator(CoordinatorInfo {
                coordinator_id: format!("coordinator-{}", peer_port),
                host: "127.0.0.1".to_string(),
                port: peer_port,
            })
            .await;

        println!("👥 Will connect to peer coordinator on port {}", peer_port);
    }

    // Start HTTP server
    {
        let coordinator_for_http = coordinator_arc.clone();
        tokio::spawn(async move {
            if let Err(e) = create_http_server(port, coordinator_for_http).await {
                eprintln!("HTTP server error: {}", e);
            }
        });
    }

    // No more message routing background task - using TCP connections

    // Wait a moment for HTTP server to start
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Conditionally spawn entities based on --entities flag
    let mut spawned_entities = Vec::new();
    if let Some(count) = auto_spawn_count {
        println!("🌱 Auto-spawning {} entities...", count);
        for i in 0..count {
            match coordinator_arc.spawn_entity().await {
                Ok(entity_id) => {
                    spawned_entities.push(entity_id);
                    println!("✅ Auto-spawned entity {} of {}", i + 1, count);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => {
                    eprintln!("❌ Failed to auto-spawn entity {}: {}", i + 1, e);
                }
            }
        }

        // Share entity directory with peers after spawning
        {
            let peers = { coordinator_arc.peer_coordinators.lock().await.clone() };
            for peer in peers {
                if let Err(e) = coordinator_arc.share_entity_directory(&peer).await {
                    eprintln!("Failed to share directory with peer: {}", e);
                }
            }
        }

        println!("✅ Auto-spawned {} entities: {:?}", spawned_entities.len(), spawned_entities);
    } else {
        println!("⏸️  No --entities flag provided. Use POST /spawn_entity to manually spawn entities.");
    }
    println!("🔗 HTTP API available on port {}", port);

    if peer_port.is_some() {
        println!("💬 Try sending cross-coordinator messages!");
        if spawned_entities.len() >= 2 {
            println!("📝 Example: curl -X POST http://localhost:{}/ask_entity/{} -H 'Content-Type: text/plain' -d 'Hello!'", port, spawned_entities[1]);
        }
    }

    // Keep running
    loop {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let entities = coordinator_arc.get_entity_list().await;
        println!("📊 Active entities: {:?}", entities);
    }
}
