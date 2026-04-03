/**
 * @license
 * Copyright 2026 Copilot Shell
 * SPDX-License-Identifier: Apache-2.0
 */

import * as path from 'node:path';
import * as os from 'node:os';
import { promises as fs } from 'node:fs';

const QWEN_DIR = '.copilot-shell';
const ALIYUN_CREDS_FILENAME = 'aliyun_creds.json';

/**
 * Default model for Aliyun auth
 */
export const ALIYUN_DEFAULT_MODEL = 'qwen3-coder-plus';

/**
 * Aliyun AK/SK credentials interface
 */
export interface AliyunCredentials {
  accessKeyId: string;
  accessKeySecret: string;
}

/**
 * Get the path to the Aliyun credentials file
 */
export function getAliyunCredsPath(): string {
  return path.join(os.homedir(), QWEN_DIR, ALIYUN_CREDS_FILENAME);
}

/**
 * Save Aliyun credentials to disk
 */
export async function saveAliyunCredentials(
  credentials: AliyunCredentials,
): Promise<void> {
  const filePath = getAliyunCredsPath();
  try {
    await fs.mkdir(path.dirname(filePath), { recursive: true });
    const credString = JSON.stringify(credentials, null, 2);
    await fs.writeFile(filePath, credString, { mode: 0o600 });
  } catch (error: unknown) {
    const errorMessage = error instanceof Error ? error.message : String(error);
    const errorCode =
      error instanceof Error && 'code' in error
        ? (error as Error & { code?: string }).code
        : undefined;

    if (errorCode === 'EACCES') {
      throw new Error(
        `Failed to save Aliyun credentials: Permission denied (EACCES). Please check permissions for \`${filePath}\`.`,
      );
    }

    throw new Error(
      `Failed to save Aliyun credentials: ${errorMessage}. Please check permissions.`,
    );
  }
}

/**
 * Load Aliyun credentials from disk
 */
export async function loadAliyunCredentials(): Promise<AliyunCredentials | null> {
  const filePath = getAliyunCredsPath();
  try {
    const content = await fs.readFile(filePath, 'utf-8');
    const credentials = JSON.parse(content) as AliyunCredentials;

    // Validate credentials structure
    if (!credentials.accessKeyId || !credentials.accessKeySecret) {
      console.warn('Invalid Aliyun credentials format in file');
      return null;
    }

    return credentials;
  } catch (error: unknown) {
    if (error instanceof Error && 'code' in error && error.code === 'ENOENT') {
      // File doesn't exist
      return null;
    }
    console.warn('Failed to load Aliyun credentials:', error);
    return null;
  }
}

/**
 * Clear Aliyun credentials from disk
 */
export async function clearAliyunCredentials(): Promise<void> {
  const filePath = getAliyunCredsPath();
  try {
    await fs.unlink(filePath);
    console.debug('Aliyun credentials cleared successfully.');
  } catch (error: unknown) {
    if (error instanceof Error && 'code' in error && error.code === 'ENOENT') {
      // File doesn't exist, already cleared
      return;
    }
    console.warn('Warning: Failed to clear Aliyun credentials:', error);
  }
}

/**
 * Check if Aliyun credentials exist
 */
export async function hasAliyunCredentials(): Promise<boolean> {
  const credentials = await loadAliyunCredentials();
  return credentials !== null;
}
