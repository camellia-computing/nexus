export function managedWorkingDirectory(executablePath: string): string {
  const path = executablePath.replaceAll('\\', '/').replace(/\/+$/, '');
  const separator = path.lastIndexOf('/');
  return separator > 0 ? path.slice(0, separator) : 'bin';
}
