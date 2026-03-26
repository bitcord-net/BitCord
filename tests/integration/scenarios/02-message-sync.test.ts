/**
 * Scenario 02 — Message sync
 *
 * Tests message_send / message_get_history on a single node and verifies
 * cross-node delivery via QUIC gossip.
 *
 * Port allocation:
 *   Node A: api=7421
 *   Node B: api=7431
 */

import { describe, it, beforeAll, afterAll } from "vitest";
import { NodeProcess } from "../helpers/NodeProcess.js";
import { BitCordTestClient } from "../helpers/BitCordTestClient.js";
import { TestLogger, pollUntil } from "../helpers/TestLogger.js";

describe("message sync", () => {
  let nodeA: NodeProcess;
  let nodeB: NodeProcess;
  let clientA: BitCordTestClient;
  let clientB: BitCordTestClient;
  const logger = new TestLogger("message-sync");

  let communityId = "";
  let channelId = "";
  let sentMessageId = "";

  beforeAll(async () => {
    nodeA = new NodeProcess({ label: "node-a-msg", apiPort: 7421 });
    await nodeA.start();
    nodeB = new NodeProcess({ label: "node-b-msg", apiPort: 7431 });
    await nodeB.start();

    clientA = new BitCordTestClient(nodeA.apiUrl);
    await clientA.connectAndWait(5_000);
    clientB = new BitCordTestClient(nodeB.apiUrl);
    await clientB.connectAndWait(5_000);

    // Set up community + channel on A.
    const community = await clientA.communityCreate({
      name: "Msg Test",
      description: "",
      seed_nodes: [],
    });
    communityId = String(community["id"]);

    const channel = await clientA.channelCreate({
      community_id: communityId,
      name: "general",
      kind: "text",
    });
    channelId = String(channel["id"]);

    // Get node A's address and join the community on B (local placeholder).
    const addrInfo = await clientA.nodeGetLocalAddrs();
    const seedAddr =
      addrInfo.listen_addrs.find(
        (a) =>
          !a.includes("0.0.0.0") &&
          !a.includes("::") &&
          !a.includes("127.0.0.1") &&
          !a.includes("[::1]")
      ) ?? addrInfo.listen_addrs.find((a) => a.length > 0) ?? "127.0.0.1:0";

    const invitePayload = JSON.stringify({
      community_id: communityId,
      name: community["name"],
      description: "",
      seed_nodes: [seedAddr].filter(Boolean),
      public_key_hex: community["public_key_hex"],
    });
    const invite = Buffer.from(invitePayload).toString("base64url");
    await clientB.communityJoin(invite);

    // Wait for channel metadata to propagate to node B via QUIC gossip.
    await pollUntil(async () => {
      const channels = await clientB.channelList(communityId);
      return channels.find((c) => String(c["id"]) === channelId) ?? null;
    }, 15_000);
  });

  afterAll(async () => {
    logger.addNodeLogs(nodeA.logBuffer);
    logger.addNodeLogs(nodeB.logBuffer);
    logger.writeReport();
    clientA.close();
    clientB.close();
    await nodeA.stop();
    await nodeB.stop();
  });

  it("node A sends a message", async () => {
    const msg = await logger.step(
      "message_send",
      () =>
        clientA.messageSend({
          community_id: communityId,
          channel_id: channelId,
          body: "hello from A",
        }),
      { community_id: communityId, channel_id: channelId }
    );
    sentMessageId = String(msg["id"]);
    if (!sentMessageId) throw new Error("message_send returned no id");
  });

  it("node B receives the message in history within 10 s", async () => {
    await logger.step("message_sync_b", () =>
      pollUntil(async () => {
        const history = await clientB.messageGetHistory({
          community_id: communityId,
          channel_id: channelId,
          limit: 20,
        });
        return history.find((m) => m["id"] === sentMessageId) ?? null;
      }, 10_000)
    );
  });

  it("node A can also retrieve its own message", async () => {
    const history = await logger.step("message_get_history_a", () =>
      clientA.messageGetHistory({
        community_id: communityId,
        channel_id: channelId,
        limit: 20,
      })
    );

    if (!history.some((m) => m["id"] === sentMessageId)) {
      throw new Error(`Message ${sentMessageId} not found in node A history`);
    }
  });

  it("node A deletes the message", async () => {
    await logger.step("message_delete", () =>
      clientA.messageDelete({
        community_id: communityId,
        channel_id: channelId,
        message_id: sentMessageId,
      })
    );
  });

  it("deleted message is tombstoned in node A history", async () => {
    const history = await logger.step("message_get_history_a_after_delete", () =>
      clientA.messageGetHistory({
        community_id: communityId,
        channel_id: channelId,
        limit: 20,
      })
    );
    const msg = history.find((m) => m["id"] === sentMessageId);
    if (!msg) throw new Error("message not found in history after delete");
    if (msg["deleted"] !== true)
      throw new Error(`expected deleted=true, got deleted=${String(msg["deleted"])}`);
  });

  it("deletion propagates to node B within 10 s", async () => {
    await logger.step("message_delete_sync_b", () =>
      pollUntil(async () => {
        const history = await clientB.messageGetHistory({
          community_id: communityId,
          channel_id: channelId,
          limit: 20,
        });
        const msg = history.find((m) => m["id"] === sentMessageId);
        return msg && msg["deleted"] === true ? msg : null;
      }, 10_000)
    );
  });
});
