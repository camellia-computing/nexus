export interface ArgumentParseResult {
  args: string[];
  error: string;
  warnings: string[];
}

export function parseArgumentLine(
  line: string,
  executable = '',
  platform = '',
): ArgumentParseResult {
  const args: string[] = [];
  const warnings: string[] = [];
  let token = '';
  let tokenStarted = false;
  let quote = '';

  for (let index = 0; index < line.length; index += 1) {
    const character = line[index];
    if (character === '\n' || character === '\r' || character === '\0') {
      return invalid('Arguments must stay on one line.');
    }
    if (quote) {
      if (character === quote) {
        quote = '';
      } else if (character === '\\' && line[index + 1] === quote) {
        token += quote;
        index += 1;
      } else {
        token += character;
      }
      tokenStarted = true;
      continue;
    }
    if (character === '"' || character === "'") {
      quote = character;
      tokenStarted = true;
    } else if (/\s/.test(character)) {
      if (tokenStarted) {
        args.push(token);
        token = '';
        tokenStarted = false;
      }
    } else if (
      character === '\\' &&
      index + 1 < line.length &&
      (/\s/.test(line[index + 1]) || line[index + 1] === '"' || line[index + 1] === "'")
    ) {
      token += line[index + 1];
      tokenStarted = true;
      index += 1;
    } else {
      token += character;
      tokenStarted = true;
    }
  }
  if (quote) return invalid(`Missing closing ${quote} quote.`);
  if (tokenStarted) args.push(token);
  if (args.length > 256) return invalid('Arguments are limited to 256 values.');
  const commandUnits = args.reduce((total, argument) => total + argument.length + 1, 0);
  if (commandUnits > 24_000) return invalid('The parsed command line is too long.');

  if (args.length > 0 && executable && sameExecutable(args[0], executable, platform)) {
    args.shift();
    warnings.push('Executable excluded from arguments.');
  } else if (executable && args[0]?.toLowerCase().endsWith('.exe')) {
    warnings.push('First value kept as argument 1.');
  }
  if (args.some((argument) => ['|', '||', '&&', '>', '>>', '<', ';'].includes(argument))) {
    warnings.push('Shell operators are treated as arguments.');
  }
  return { args, error: '', warnings };

  function invalid(error: string): ArgumentParseResult {
    return { args: [], error, warnings: [] };
  }
}

export function formatArgumentLine(args: string[]): string {
  return args.map(formatArgument).join(' ');
}

function formatArgument(argument: string): string {
  if (!argument) return '""';
  return [...argument]
    .map((character) => (/\s/.test(character) || character === '"' || character === "'" ? `\\${character}` : character))
    .join('');
}

function sameExecutable(value: string, executable: string, platform: string): boolean {
  const normalize = (path: string) => path.replaceAll('\\', '/').replace(/\/$/, '');
  const left = normalize(value);
  const right = normalize(executable);
  const leftName = left.split('/').at(-1) ?? left;
  const rightName = right.split('/').at(-1) ?? right;
  const leftContainsPath = left.includes('/');
  return platform === 'Windows'
    ? left.toLowerCase() === right.toLowerCase() ||
        (!leftContainsPath && leftName.toLowerCase() === rightName.toLowerCase())
    : left === right || (!leftContainsPath && leftName === rightName);
}
