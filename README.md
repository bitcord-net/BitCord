# BitCord

**Note: BitCord is currently in development. Expect breaking changes, experimental features, and incomplete documentation.** 

BitCord is an encrypted, peer-to-peer communication platform designed as a privacy-focused alternative to centralized chat platforms. It operates on a client-node architecture where the network is maintained by participants rather than a central authority. All communications are end-to-end encrypted, and nodes act as zero-knowledge relays that store only opaque, encrypted data blobs.

## Working Features

*   **Distributed Network:** Operates without central servers using a client-to-node architecture over the QUIC transport protocol.
*   **GUI Client:** A cross-platform desktop application built with Tauri and React that includes an embedded mini-node for local operation.
*   **Headless Node:** A standalone server binary for running always-on nodes on VPS or home hardware to host communities and mailboxes.
*   **End-to-End Encryption:** All channel messages and Direct Messages are encrypted using XChaCha20-Poly1305. Identity is managed via Ed25519 keypairs.
*   **Direct Messages (DMs):** Supports store-and-forward delivery via node mailboxes, allowing users to receive messages sent while they were offline.
*   **Communities:** Users can create decentralized communities with per-channel encrypted logs. Communities can be hosted locally or migrated to dedicated seed nodes.
*   **Invite System:** Secure community discovery using Base64URL-encoded invite links that bundle community metadata, connection information, and TLS certificate fingerprints for certificate pinning.
*   **Roles and Permissions:** Community members can be assigned roles (Admin or Moderator) that grant moderation privileges. See the [Roles and Permissions](#roles-and-permissions) section for details.

## Planned Features
*   **Identity Sharing**: Share your ID across devices.
*   **Better Android Support**: While the mobile build works, it needs UI/UX improvements.
*   **Custom Emoji:** Support for community-specific reaction and message emojis.
*   **GIF Integration:** Built-in support for searching and sharing animated GIFs.
*   **Voice Chat:** Real-time encrypted voice communication.
*   **Full Multi-Admin Key Distribution:** Allowing promoted admins to wrap channel keys for new members, removing the dependency on the community creator being online.

## E2EE Key Distribution & Admin Availability

BitCord communities use per-channel encryption keys that are distributed exclusively by the community admin (the node whose Ed25519 keypair created the community). This is a deliberate design choice: relay/seed nodes store only encrypted ciphertext and never hold plaintext channel keys, preserving end-to-end encryption even against a compromised relay.

### How key distribution works

When a new member joins a community, the relay stores their membership record. The admin node is responsible for wrapping (encrypting) the channel key specifically for that member's X25519 public key and broadcasting the result. The new member then decrypts their copy of the key locally.

### Admin availability requirement

Because only the admin can wrap channel keys, **the admin node must come online at least once after a new member joins** for that member to receive their decryption key. Until then, the new member can connect to the relay and see the community structure, but cannot decrypt message history or send encrypted messages.

Once the admin connects, it automatically detects any members whose keys have not yet been wrapped, wraps them, and broadcasts the updated channel manifests. From that point on, the new member's client will decrypt and display history on next connect — even if the admin goes offline again immediately after.

**In practice:** For communities hosted on an always-on headless node where the admin account is the node's own identity, this happens transparently. For communities created from a desktop client where the admin is a regular user, that user should remain online during the onboarding window for new members, or come back online periodically.

### Relay nodes are zero-knowledge

Relay/seed nodes intentionally do **not** store plaintext channel keys. They store encrypted message logs and wrapped-key manifests, but cannot decrypt any content. This is why key distribution requires the admin: there is no server-side authority that can issue keys on the admin's behalf.

## TLS Certificate Pinning

BitCord uses TLS certificate pinning to ensure that clients connect to the correct node, even without a certificate authority. Each node generates a self-signed TLS certificate deterministically from its Ed25519 signing key. The SHA-256 fingerprint of this certificate is embedded in invite links and used for certificate pinning.

### How it works

*   When an admin creates a community with external seed nodes, the seed node's TLS certificate fingerprint must be provided.
*   When an admin generates an invite link, the seed node's TLS certificate fingerprint is included in the invite payload.
*   When a user joins a community via an invite link, the fingerprint is extracted and used to verify the identity of the seed node. Invites with a present but malformed fingerprint are rejected.
*   The fingerprint is persisted locally so that reconnections (including automatic seed peer reconnects) continue to enforce the pinned certificate.
*   If the seed node's certificate changes (e.g. the node is reinstalled), the client will refuse to connect due to a fingerprint mismatch, preventing man-in-the-middle attacks.

### Changing seed nodes

When an admin changes the community's seed node via `community_update_manifest`, the new seed node's TLS certificate fingerprint must be provided (`seed_fingerprint_hex`). The update is rejected without it, ensuring that certificate pinning is never silently bypassed during a seed migration.

### Bootstrap and DHT connections

Connections to global bootstrap seed nodes (configured in the node's config file) and dynamically discovered DHT peers use Trust-On-First-Use (TOFU) mode, where any certificate is accepted. These are the only connection types where TOFU is permitted — all community seed node connections require a pinned fingerprint.

## Roles and Permissions

Every community member has one of three roles: **Community Creator**, **Admin**, **Moderator**, or **Member**. Roles are stored in each node's member list and propagated via signed gossip.

### Community Creator

The creator is the node whose Ed25519 keypair generated the community. Their identity is permanently embedded in the community manifest and cannot be changed. The creator cannot be demoted.

The creator can do everything an Admin can, plus:

| Action | Notes |
|---|---|
| Create channels | Requires the creator's signing key to produce a valid manifest |
| Delete channels | Same — manifest re-signing is cryptographically creator-only |
| Rotate channel encryption keys | Verified against the community public key on receipt |
| Update community name / description | Manifest signature required |
| Delete the community | Broadcasts a signed tombstone manifest |
| Wrap channel keys for new members | Only the creator's node performs this automatically on join |

### Admin

Admins are members promoted by the community creator. Their role is tracked in the member list and propagated to all peers via signed gossip when assigned.

| Action | Can do? |
|---|---|
| Post in announcement channels | Yes |
| Kick members | Yes |
| Ban members | Yes |
| Promote/demote other members | Yes (cannot demote the creator) |
| Create / delete channels | No — requires creator's signing key |
| Update community manifest | No — requires creator's signing key |
| Wrap channel keys for new members | No — requires creator's signing key |

### Moderator

Moderators are members promoted by the creator or an admin.

| Action | Can do? |
|---|---|
| Post in announcement channels | Yes |
| Kick members | Yes |
| Ban members | No |
| Promote/demote other members | No |

### Key Distribution Limitation

Because channel key wrapping is tied to the creator's Ed25519 keypair (not the role system), promoted admins cannot onboard new members on the creator's behalf. This is a known limitation — see **Full Multi-Admin Key Distribution** in Planned Features above, and the [E2EE Key Distribution & Admin Availability](#e2ee-key-distribution--admin-availability) section for background.

## Network Architecture

BitCord utilizes a decentralized architecture where every participant contributes to the network's resilience and privacy.

*   **Gossip-Relay System:** Real-time events, such as message reactions, presence updates, and community changes, are propagated through a peer-to-peer gossip layer. Every node acts as a relay to ensure updates reach all online participants without a central coordinator.
*   **Iterative Data Sync:** Community manifests and message history are fetched directly from peers or dedicated seed nodes. Nodes synchronize encrypted event logs to stay up to date with the latest state.
*   **Decentralized Identity:** Users own their identity via Ed25519 keypairs. There is no central identity provider; authentication and encryption are performed peer-to-peer using these cryptographic keys.
*   **Mailbox Routing (DHT):** A Kademlia-style Distributed Hash Table (DHT) is used to route Direct Messages. It maps user IDs to the nodes currently hosting their offline mailboxes, enabling reliable asynchronous delivery even when users are not concurrently online.

## How to Use

### Desktop Application
The desktop app provides all features, including the ability to join communities, send DMs, and host small communities from your own machine.

### Headless Mode
For users who want to provide infrastructure for the network or host permanent communities, the headless node can be run on a server.
```bash
cargo run -p bitcord-node -- [FLAGS]
```

### Configuration & Flags
BitCord can be configured via command-line flags or environment variables:

*   `BITCORD_PASSPHRASE`: Used to decrypt your identity and local data files. If not set, you will be prompted interactively.
*   `BITCORD_JOIN_PASSWORD`: Sets a password requirement for new communities to register on a specific node.
*   `--join-password`: CLI equivalent to the environment variable.

### Invite Links
Communities are shared via a Base64URL-encoded URI format:
`bitcord://join/<base64url_payload>`

The payload contains the community ID, name, description, and the addresses of initial seed nodes required to bootstrap the connection.

## How to Build

### Prerequisites
*   **Rust:** 1.87 or newer
*   **Node.js:** 22 or newer
*   **System Dependencies:** Microsoft C++ Build Tools and WebView2 (for Windows) or equivalent webkit/build tools for Linux/macOS.

### Build Steps

1.  **Install Frontend Dependencies:**
    ```bash
    cd app
    npm install
    ```

2.  **Development Build:**
    To run the desktop application in development mode with hot-reloading:
    ```bash
    cd app
    npm run tauri:dev
    ```

### Production Build
To generate a platform-native installer:
```bash
cd app
npm run tauri:build
```
The resulting installer will be located in the `target/release/bundle/` directory.

> **Important:** Do not build the desktop app with `cargo build --release` directly. The Tauri CLI (`npm run tauri:build`) sets compile-time flags that switch the webview from the Vite dev server URL to the embedded production frontend. Running `cargo build --release` bypasses this and produces a binary that tries to connect to `http://localhost:1420` (the dev server) instead of loading the bundled UI, resulting in an `ERR_CONNECTION_REFUSED` error.

### Building the Headless Node
```bash
cargo build --release -p bitcord-node
```

### Android Development
Building for Android requires setting up the Android SDK and Rust mobile targets.

#### 1. Prerequisites
*   **Android Studio**: Install and use the SDK Manager to add:
    *   Android SDK Platform (API 33+)
    *   Android SDK Build-Tools
    *   NDK (Side by side) (v25 or v26 recommended)
    *   Android SDK Command-line Tools
*   **JDK 17**: Required for the Android build process.
*   **Rust Targets**:
    ```bash
    rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
    ```

#### 2. Environment Variables
Ensure the following are set in your shell profile or system settings:
*   `JAVA_HOME`: Path to your JDK 17.
*   `ANDROID_HOME`: Path to your Android SDK (e.g., `%LOCALAPPDATA%\Android\Sdk` on Windows).
*   `NDK_HOME`: Path to the specific NDK version (e.g., `$ANDROID_HOME/ndk/26.x.x`).

#### 3. Android Commands
All commands should be run from the `app/` directory:

*   **Initialize Android Project**:
    ```bash
    npm run tauri:android:init
    ```
*   **Run on Emulator/Device**:
    ```bash
    npm run tauri:android:dev
    ```
*   **Build Release APK**:
    ```bash
    npm run tauri:android:build
    ```


