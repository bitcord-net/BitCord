/**
 * Scenario 04 — Invite security
 *
 * Verifies that:
 *   1. `community_generate_invite` returns a signed, pinned invite link
 *      (sig_hex + cert_fingerprint_hex present and correctly sized).
 *   2. `community_join` rejects an unsigned invite with an explicit error.
 *   3. `community_join` rejects an invite carrying a bad signature.
 *
 * All tests run against a single spawned node — no P2P required.
 *
 * Port allocation:
 *   Node: api=7451
 */

import { describe, it, beforeAll, afterAll, expect } from "vitest";
import { NodeProcess } from "../helpers/NodeProcess.js";
import { BitCordTestClient, RpcError } from "../helpers/BitCordTestClient.js";

describe("invite security", () => {
  let node: NodeProcess;
  let client: BitCordTestClient;
  let communityId: string;
  let communityPublicKeyHex: string;

  beforeAll(async () => {
    node = new NodeProcess({ label: "node-invite-sec", apiPort: 7451 });
    await node.start();
    client = new BitCordTestClient(node.apiUrl);
    await client.connectAndWait(5_000);

    const community = await client.communityCreate({
      name: "Security Test",
      description: "",
      seed_nodes: [],
    });
    communityId = String(community["id"]);
    communityPublicKeyHex = String(community["public_key_hex"]);
  });

  afterAll(async () => {
    client.close();
    await node.stop();
  });

  it("community_generate_invite returns a link with sig_hex and cert_fingerprint_hex", async () => {
    const link = await client.communityGenerateInvite(communityId);

    expect(typeof link).toBe("string");
    expect(link.startsWith("bitcord://join/")).toBe(true);

    const b64 = link.slice("bitcord://join/".length);
    const decoded = Buffer.from(b64, "base64url").toString("utf8");
    const payload = JSON.parse(decoded) as Record<string, unknown>;

    expect(typeof payload["sig_hex"]).toBe("string");
    expect((payload["sig_hex"] as string).length).toBe(128);

    expect(typeof payload["cert_fingerprint_hex"]).toBe("string");
    expect((payload["cert_fingerprint_hex"] as string).length).toBe(64);
  });

  it("community_join rejects an invite with no sig_hex", async () => {
    // Use a ULID that won't exist locally so we don't hit "already a member".
    const fakeCommunityId = "01JZZZZZZZZZZZZZZZZZZZZZZZ";
    const unsigned = Buffer.from(
      JSON.stringify({
        community_id: fakeCommunityId,
        name: "Fake",
        description: "",
        seed_nodes: [],
        public_key_hex: communityPublicKeyHex,
        // sig_hex intentionally absent
      })
    ).toString("base64url");

    await expect(client.communityJoin(unsigned)).rejects.toSatisfy(
      (err: unknown) =>
        err instanceof RpcError && err.message.includes("admin signature required")
    );
  });

  it("community_join rejects an invite with a bad signature", async () => {
    const fakeCommunityId = "01JZZZZZZZZZZZZZZZZZZZZZZX";
    const badSig = Buffer.from(
      JSON.stringify({
        community_id: fakeCommunityId,
        name: "Fake",
        description: "",
        seed_nodes: [],
        public_key_hex: communityPublicKeyHex,
        sig_hex: "ab".repeat(64), // 128 hex chars but not a valid signature
      })
    ).toString("base64url");

    await expect(client.communityJoin(badSig)).rejects.toSatisfy(
      (err: unknown) =>
        err instanceof RpcError && err.message.includes("signature verification failed")
    );
  });
});
