
## coordinator for distributed entities

## requirements
- rust toolchain
- `yaboring/entity-core` checked out at the same directory level as this project, and compiled with `cargo build --release`

## quick start

```bash
# Build both components
cd ../entity-core && cargo build --release
cd ../coordinator && cargo build --release

# Start coordinator with auto-spawned entities
cargo run -- 9000 --entities 3

# In another terminal, connect a second coordinator
cargo run -- 9001 9000 --entities 2
```

This creates a 2-coordinator network managing 5 entities. Entities can communicate across coordinators through HTTP message routing.

## HTTP API

### Entity Management
```bash
# Spawn new entity
curl -X POST http://localhost:9000/spawn_entity \
  -H 'Content-Type: application/json' -d '{}'

# List active entities
curl http://localhost:9000/entities
```

### Entity Communication (Message Proxy)
```bash
# Send message to any entity (local or remote)
curl -X POST http://localhost:9000/ask_entity/{entity_id} \
  -H 'Content-Type: text/plain' \
  -d 'Hello!'
```

## Command Line Usage

```bash
# Basic coordinator (HTTP API only)
./coordinator 9000

# Coordinator with entities
./coordinator 9000 --entities 3

# Networked coordinator
./coordinator 9001 9000 --entities 2  # connects to coordinator on 9000
```

## Development Notes
- Requires `entity-core` binary at `../entity-core/target/release/entity-core`
- Entities get OS-assigned TCP ports, coordinators use specified HTTP ports (watch for collision or exhaustion)
- Entity directory only syncs automatically from peer to main coordinator, and only once after peer boot
