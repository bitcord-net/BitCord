/**
 * Scenario 05 — Direct (mailbox-less) DM delivery
 *
 * Verifies that a DM sent via `dm_send` reaches the recipient as a `dm_new`
 * push event when both nodes are live and directly connected — no mailbox
 * store-and-forward involved.
 *
 * Setup:
 *   1. Node A starts and exposes its QUIC listen address.
 *   2. Node B starts with Node A as a seed peer, establishing a direct QUIC
 *      connection before any community join.
 *   3. A creates a community whose invite carries A's address; B joins, causing
 *      B's membership record (including B's x25519 public key) to sync to A.
 *   4. Once A's member list contains B, A sends a DM.  Because B is in A's
 *      connected-peers map, the message takes the direct path — no mailbox.
 *
 * Port allocation:
 *   Node A: api=7461
 *   Node B: api=7471
 */

import { describe, it, beforeAll, afterAll, expect } from "vitest";
import { NodeProcess } from "../helpers/NodeProcess.js";
import { BitCordTestClient, PushEvent } from "../helpers/BitCordTestClient.js";
import { TestLogger, pollUntil } from "../helpers/TestLogger.js";

describe("direct DM delivery", () => {
  let nodeA: NodeProcess;
  let nodeB: NodeProcess;
  let clientA: BitCordTestClient;
  let clientB: BitCordTestClient;
  const logger = new TestLogger("dm-direct");

  let communityId = "";
  let peerIdB = "";
  let sentMessageId = "";

  beforeAll(async () => {
    // ── Start node A and discover its QUIC listen address ─────────────────────
    nodeA = new NodeProcess({ label: "node-a-dm", apiPort: 7461 });
    await nodeA.start();
    clientA = new BitCordTestClient(nodeA.apiUrl);
    await clientA.connectAndWait(5_000);

    // Poll until the node reports a concrete (non-wildcard) listen address.
    const nodeAAddr = await pollUntil(async () => {
      const data = await clientA.nodeGetLocalAddrs();
      return (
        data.listen_addrs.find(
          (a) => !a.includes("0.0.0.0") && !a.includes("::")
        ) ??
        // Fallback: accept loopback so local-only CI environments work.
        data.listen_addrs.find((a) => a.length > 0) ??
        null
      );
    }, 8_000);

    // ── Start node B pre-seeded with A's address ──────────────────────────────
    // This guarantees a direct QUIC peer connection before any community join.
    nodeB = new NodeProcess({
      label: "node-b-dm",
      apiPort: 7471,
      seedNodes: [nodeAAddr],
    });
    await nodeB.start();
    clientB = new BitCordTestClient(nodeB.apiUrl);
    await clientB.connectAndWait(5_000);

    // ── Create community on A (seedless) ─────────────────────────────────────
    // Node B is already transport-connected to A via the seedNodes config, so
    // gossip will propagate without needing seed_nodes in the manifest (which
    // would require a TLS fingerprint we don't have at create-time).
    const community = await clientA.communityCreate({
      name: "DM Test Community",
      description: "",
      seed_nodes: [],
    });
    communityId = String(community["id"]);

    const invite = await clientA.communityGenerateInvite(communityId);
    await clientB.communityJoin(invite);

    // ── Get B's peer_id ───────────────────────────────────────────────────────
    const identityB = await clientB.identityGet();
    peerIdB = String(identityB["peer_id"]);
    if (!peerIdB) throw new Error("node B has no peer_id");

    // ── Wait for B's member record to appear on A ─────────────────────────────
    // This confirms A has B's x25519 public key and the direct delivery path
    // will be able to encrypt the DM envelope correctly.
    await pollUntil(async () => {
      const members = await clientA.memberList(communityId);
      return members.find((m) => String(m["user_id"]) === peerIdB) ?? null;
    }, 20_000);
  }, 60_000);

  afterAll(async () => {
    logger.addNodeLogs(nodeA.logBuffer);
    logger.addNodeLogs(nodeB.logBuffer);
    logger.writeReport();
    clientA.close();
    clientB.close();
    await nodeA.stop();
    await nodeB.stop();
  });

  it("B receives a dm_new push event when A sends a direct DM", async () => {
    // Subscribe on B *before* sending so we cannot miss the event.
    const dmReceived: Promise<PushEvent> = clientB.waitForEvent("dm_new", 15_000);

    const sent = await logger.step("dm_send", () =>
      clientA.dmSend(peerIdB, "hello via direct path")
    );
    sentMessageId = String(sent["id"]);
    expect(sentMessageId).toBeTruthy();

    const event = await logger.step("dm_new_received", () => dmReceived);

    // PushEvent shape: { type: "dm_new", data: { message: DmMessageInfo } }
    const msgData = (event["data"] as Record<string, unknown>)["message"] as Record<string, unknown>;
    expect(String(msgData["body"])).toBe("hello via direct path");
    // author_id is the sender's peer_id (A's identity).
    expect(String(msgData["author_id"])).toBe(
      String((await clientA.identityGet())["peer_id"])
    );
  });

  it("A's dm_get_history contains the sent message", async () => {
    const history = await logger.step("dm_get_history_sender", () =>
      clientA.dmGetHistory(peerIdB)
    );

    const msg = history.find((m) => m["id"] === sentMessageId);
    expect(msg).toBeTruthy();
    expect(String(msg!["body"])).toBe("hello via direct path");
    // From A's perspective the message is outgoing: author_id === A's peer_id.
    const peerIdA = String((await clientA.identityGet())["peer_id"]);
    expect(String(msg!["author_id"])).toBe(peerIdA);
  });

  it("B's dm_get_history contains the received message", async () => {
    // B stores incoming DMs keyed by the sender's peer_id.
    const senderPeerIdA = String((await clientA.identityGet())["peer_id"]);

    const history = await logger.step("dm_get_history_recipient", () =>
      clientB.dmGetHistory(senderPeerIdA)
    );

    const msg = history.find((m) => m["id"] === sentMessageId);
    expect(msg).toBeTruthy();
    expect(String(msg!["body"])).toBe("hello via direct path");
    // From B's perspective the message is incoming: author_id is A (the sender).
    expect(String(msg!["author_id"])).toBe(senderPeerIdA);
  });
});
