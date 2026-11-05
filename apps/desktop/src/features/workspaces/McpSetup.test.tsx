import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { McpSetup } from "./McpSetup";
const mocks = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));
const setup = { provider: "codex", path: "/test/.codex/config.toml", config: "[mcp_servers.orvex]", configured: false, conflict: false };
beforeEach(() => {
  mocks.invoke.mockReset();
  HTMLDialogElement.prototype.showModal = function () { this.setAttribute("open", ""); };
  mocks.invoke.mockResolvedValue(setup);
});
afterEach(cleanup);
function show(clients: { id: string; name: string; version: string; processId: number; connectedAt: number }[] = []) {
  return render(<McpSetup status={{ clients, ignoredRecords: 0 }} statusError={null} refresh={vi.fn()} onClose={vi.fn()} />);
}
it("only installs after a click and distinguishes saved configuration from a connection", async () => {
  show();
  const add = await screen.findByRole("button", { name: "Add to Codex" });
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
  mocks.invoke.mockResolvedValue({ ...setup, configured: true });
  fireEvent.click(add);
  await screen.findByText(/Orvex added to Codex/);
  expect(mocks.invoke).toHaveBeenCalledWith("install_mcp_setup", { provider: "codex" });
  expect(screen.getByText("Waiting for a live MCP connection")).toBeTruthy();
});
it("blocks an existing conflicting entry", async () => {
  mocks.invoke.mockResolvedValue({ ...setup, conflict: true });
  show();
  expect(await screen.findByRole("button", { name: "Add to Codex" })).toHaveProperty("disabled", true);
  expect(mocks.invoke).toHaveBeenCalledTimes(1);
});
it("shows write errors and allows another attempt", async () => {
  show();
  const add = await screen.findByRole("button", { name: "Add to Codex" });
  mocks.invoke.mockRejectedValueOnce(new Error("Permission denied"));
  fireEvent.click(add);
  expect((await screen.findByRole("alert")).textContent).toContain("Permission denied");
  expect(add).toHaveProperty("disabled", false);
  mocks.invoke.mockResolvedValue({ ...setup, configured: true });
  fireEvent.click(add);
  expect(await screen.findByRole("button", { name: "Added to client ✓" })).toHaveProperty("disabled", true);
});
it("does not mistake Copilot for a live Pi connection", async () => {
  show([{ id: "test", name: "copilot", version: "1", processId: 1, connectedAt: 0 }]);
  fireEvent.click(await screen.findByRole("button", { name: /Pi Coding agent/ }));
  expect(screen.getByText("Waiting for a live MCP connection")).toBeTruthy();
});
