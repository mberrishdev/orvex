import { spawnSync } from 'node:child_process';
import { mkdir, copyFile, chmod } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const args = process.argv.slice(2);
if (args.length && (args.length !== 2 || args[0] !== '--prefix')) {
  throw new Error('Usage: pnpm install:cli [--prefix <directory>]');
}
const prefix = args.length ? resolve(args[1]) : join(homedir(), '.local');
const result = spawnSync('cargo', ['build', '--locked', '--package', 'orvex-desktop', '--bin', 'orvex', '--features', 'custom-protocol'], {
  cwd: root, stdio: 'inherit',
  env: { ...process.env, CARGO_PROFILE_DEV_DEBUG: '0', CARGO_INCREMENTAL: '0' },
});
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);
const destination = join(prefix, 'bin', process.platform === 'win32' ? 'orvex.exe' : 'orvex');
await mkdir(dirname(destination), { recursive: true });
const target = process.env.CARGO_TARGET_DIR ? resolve(root, process.env.CARGO_TARGET_DIR) : join(root, 'target');
// Rename on install so a running MCP process can continue using its old binary.
const temporary = `${destination}.${process.pid}.tmp`;
await copyFile(join(target, 'debug', process.platform === 'win32' ? 'orvex.exe' : 'orvex'), temporary);
await chmod(temporary, 0o755);
const { rename } = await import('node:fs/promises');
await rename(temporary, destination);
console.log(`Installed ${destination}\nUse this absolute command in your MCP client, with argument: mcp`);
