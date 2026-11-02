import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { resolve } from 'node:path';

const binary = resolve(process.env.ORVEX_TEST_BINARY ?? 'target/debug/orvex');
const args = process.argv.slice(2);
if (args.length && (args.length !== 2 || args[0] !== '--workspace')) {
  throw new Error('Usage: pnpm check:mcp [--workspace <running-workspace-id>]');
}
function client() {
  const child = spawn(binary, ['mcp'], { stdio: ['pipe', 'pipe', 'inherit'] });
  let id = 0;
  const pending = new Map();
  const lines = createInterface({ input: child.stdout });
  lines.on('line', line => {
    const response = JSON.parse(line); // any non-protocol stdout fails this check
    const receiver = pending.get(response.id);
    if (receiver) { pending.delete(response.id); clearTimeout(receiver.timer); receiver.resolve(response); }
  });
  child.on('exit', () => {
    for (const { reject, timer } of pending.values()) { clearTimeout(timer); reject(new Error('MCP exited')); }
    pending.clear();
  });
  const request = (method, params = {}) => new Promise((resolve, reject) => {
    const requestId = ++id;
    const timer = setTimeout(() => { pending.delete(requestId); reject(new Error(`Timed out: ${method}`)); }, 45000);
    pending.set(requestId, { resolve, reject, timer });
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', id: requestId, method, params })}\n`);
  });
  const tool = async (name, values) => {
    const response = await request('tools/call', { name, arguments: values });
    assert(!response.error, JSON.stringify(response));
    assert(!response.result.isError, JSON.stringify(response.result));
    return response.result.structuredContent;
  };
  const hello = async () => {
    const response = await request('initialize', { protocolVersion: '2025-11-25', capabilities: {}, clientInfo: { name: 'orvex-check', version: '1' } });
    assert.equal(response.result.serverInfo.name, 'orvex');
    child.stdin.write(`${JSON.stringify({ jsonrpc: '2.0', method: 'notifications/initialized' })}\n`);
  };
  return { request, tool, hello, close: () => child.stdin.end() };
}

let connection = client();
let sessionId;
try {
  await connection.hello();
  const tools = await connection.request('tools/list');
  assert.equal(tools.result.tools.length, 15);
  assert((await connection.request('ping')).result);
  const denied = await connection.request('tools/call', { name: 'workspace_start', arguments: { workspaceId: 'nonexistent', network: false } });
  assert.equal(denied.result.isError, true);
  assert.match(denied.result.content[0].text, /network/);
  console.log('MCP handshake, discovery, ping and denied network grant passed.');

  if (args.length) {
    const workspaceId = args[1];
    const status = await connection.tool('workspace_status', { workspaceId });
    assert.equal(status.vmState, 'running', 'The selected VM must already be running.');
    const location = await connection.tool('workspace_exec', { workspaceId, command: ['pwd'] });
    assert.equal(location.exitCode, 0);
    assert(location.stdout.trim().endsWith(`/orvex/workspaces/${workspaceId}`));
    const bounded = await connection.tool('workspace_exec', { workspaceId, command: ['bash', '-c', 'head -c 70000 /dev/zero'] });
    assert.equal(bounded.truncated, true);
    assert.equal(bounded.stdout.length, 65536);
    const timeout = await connection.tool('workspace_exec', { workspaceId, command: ['sleep', '3'], timeoutSeconds: 1 });
    assert.equal(timeout.timedOut, true);
    const session = await connection.tool('command_start', { workspaceId, command: ['bash', '-c', "printf 'orvex-mcp-check\\n'; sleep 30"] });
    sessionId = session.sessionId;
    connection.close();
    connection = client();
    await connection.hello();
    const running = await connection.tool('command_status', { workspaceId, sessionId });
    assert.equal(running.state, 'running');
    assert.match(running.logs, /orvex-mcp-check/);
    const stopped = await connection.tool('command_stop', { workspaceId, sessionId });
    assert.equal(stopped.state, 'cancelled');
    console.log('Guest checkout, bounded output, timeout, session persistence, logs and cancellation passed.');
  }
} finally {
  if (sessionId) {
    await connection.tool('command_stop', { workspaceId: args[1], sessionId }).catch(() => {});
  }
  connection.close();
}
