import { describe, expect, it } from "vitest";

import { GET } from "./route";

describe("GET /api/ping", () => {
  it("responds with pong payload", async () => {
    const response = GET();
    expect(response.status).toBe(200);
    const body = (await response.json()) as {
      message: string;
      runtime: string;
      timestamp: number;
    };
    expect(body.message).toBe("pong");
    expect(body.runtime).toBe("nexide");
    expect(typeof body.timestamp).toBe("number");
  });
});
