/**
 * Scenario 03 — Channel management
 *
 * Tests the channel_create / channel_list / channel_delete RPC methods on a
 * single node (no P2P required).
 *
 * Port allocation:
 *   Node A: api=7441
 */

import { describe, it, beforeAll, afterAll, expect } from "vitest";
import { NodeProcess } from "../helpers/NodeProcess.js";
import { BitCordTestClient } from "../helpers/BitCordTestClient.js";
import { TestLogger } from "../helpers/TestLogger.js";

describe("channel management", () => {
  let nodeA: NodeProcess;
  let clientA: BitCordTestClient;
  const logger = new TestLogger("channel-management");

  let communityId = "";

  beforeAll(async () => {
    nodeA = new NodeProcess({ label: "node-a-ch", apiPort: 7441 });
    await nodeA.start();

    clientA = new BitCordTestClient(nodeA.apiUrl);
    await clientA.connectAndWait(5_000);

    const community = await clientA.communityCreate({
      name: "Channel Test",
      description: "",
      seed_nodes: [],
    });
    communityId = String(community["id"]);
  });

  afterAll(async () => {
    logger.addNodeLogs(nodeA.logBuffer);
    logger.writeReport();
    clientA.close();
    await nodeA.stop();
  });

  it("creates three channels", async () => {
    await logger.step("channel_create_general", () =>
      clientA.channelCreate({ community_id: communityId, name: "general", kind: "text" })
    );
    await logger.step("channel_create_random", () =>
      clientA.channelCreate({ community_id: communityId, name: "random", kind: "text" })
    );
    await logger.step("channel_create_announcements", () =>
      clientA.channelCreate({
        community_id: communityId,
        name: "announcements",
        kind: "announcement",
      })
    );
  });

  it("channel_list returns all three channels", async () => {
    const channels = await logger.step("channel_list", () =>
      clientA.channelList(communityId)
    );

    const names = channels.map((c) => c["name"]);
    expect(names).toContain("general");
    expect(names).toContain("random");
    expect(names).toContain("announcements");
    expect(channels).toHaveLength(3);
  });

  it("deletes the random channel", async () => {
    const channels = await clientA.channelList(communityId);
    const randomCh = channels.find((c) => c["name"] === "random");
    if (!randomCh) throw new Error("random channel not found");

    const ok = await logger.step(
      "channel_delete",
      () =>
        clientA.channelDelete({
          community_id: communityId,
          channel_id: String(randomCh["id"]),
        }),
      { channel_id: String(randomCh["id"]) }
    );
    expect(ok).toBe(true);
  });

  it("channel_list shows only two channels after deletion", async () => {
    const channels = await logger.step("channel_list_after_delete", () =>
      clientA.channelList(communityId)
    );

    const names = channels.map((c) => c["name"]);
    expect(channels).toHaveLength(2);
    expect(names).not.toContain("random");
    expect(names).toContain("general");
    expect(names).toContain("announcements");
  });

  it("node metrics are available", async () => {
    const metrics = await logger.step("node_get_metrics", () =>
      clientA.nodeGetMetrics()
    );
    expect(metrics).toHaveProperty("connected_peers");
    expect(metrics).toHaveProperty("uptime_secs");
    expect(metrics).toHaveProperty("disk_usage_mb");
  });
});
