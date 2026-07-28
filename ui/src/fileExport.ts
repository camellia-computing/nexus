import { save } from '@tauri-apps/plugin-dialog';
import { writeTextFile } from '@tauri-apps/plugin-fs';
import { isNativeHost } from './runtime';

function downloadPreviewFile(filename: string, contents: string) {
  const blob = new Blob([contents], { type: 'application/json;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = filename;
  anchor.hidden = true;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

export async function saveJsonFile(filename: string, value: unknown) {
  const contents = `${JSON.stringify(value, null, 2)}\n`;
  if (!isNativeHost()) {
    downloadPreviewFile(filename, contents);
    return true;
  }

  try {
    const path = await save({
      defaultPath: filename,
      filters: [{ name: 'JSON', extensions: ['json'] }],
    });
    if (!path) return false;
    await writeTextFile(path, contents);
    return true;
  } catch {
    throw {
      code: 'STORAGE',
      message: 'The export file could not be saved.',
      details: '',
    };
  }
}
