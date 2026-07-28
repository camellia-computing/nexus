import type { ArgumentParseResult } from '../../arguments';

export const MAX_CONFIG_SOURCES_PER_PROGRAM = 50;

export function effectiveConfigSourceLimit(licenseLimit?: number): number {
  return typeof licenseLimit === 'number' && Number.isSafeInteger(licenseLimit) && licenseLimit >= 0
    ? Math.min(licenseLimit, MAX_CONFIG_SOURCES_PER_PROGRAM)
    : MAX_CONFIG_SOURCES_PER_PROGRAM;
}

export function enrichConfigurationArguments(
  result: ArgumentParseResult,
  flags: readonly string[],
  managedConfiguration: boolean,
): ArgumentParseResult {
  if (result.error) return result;
  for (const [index, argument] of result.args.entries()) {
    if (
      managedConfiguration &&
      flags.some((flag) => argument === flag || argument.startsWith(`${flag}=`))
    ) {
      return {
        ...result,
        error: 'Configuration path arguments are unavailable in managed mode.',
      };
    }
    if (flags.includes(argument) && !result.args[index + 1]) {
      return { ...result, error: `${argument} requires a following configuration path.` };
    }
    if (flags.some((flag) => argument === `${flag}=`)) {
      return { ...result, error: `${argument} requires a configuration path after “=”.` };
    }
  }
  return result;
}

export function hasConfigurationArgument(
  flags: readonly string[],
  args: readonly string[],
): boolean {
  return args.some((argument) =>
    flags.some((flag) => argument === flag || argument.startsWith(`${flag}=`)),
  );
}
