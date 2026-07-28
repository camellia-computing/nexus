<script lang="ts">
  import ErrorNotice from '../../ErrorNotice.svelte';
  import { t } from '../../i18n';
  import Icon from '../../lib/components/Icon.svelte';
  import ResizeSeparator from '../../lib/components/ResizeSeparator.svelte';
  import type { ErrorInfo } from '../../api';
  import type {
    XrayBalancerInfo,
    XrayDashboard,
    XrayDashboardSnapshot,
    XrayOnlineUser,
  } from '../../types';

  type XrayTrafficSort = 'scope' | 'tag' | 'uplink' | 'downlink';
  type XrayTrafficRow = {
    scope: string;
    tag: string;
    uplink: number;
    downlink: number;
  };

  export let snapshot: XrayDashboardSnapshot | null;
  export let dashboard: XrayDashboard | undefined;
  export let error: ErrorInfo | null;
  export let runtimeStateLabel: string;
  export let running: boolean;
  export let canRefresh: boolean;
  export let manualRefreshing: boolean;
  export let routingBusyTag: string;
  export let loggerBusy: boolean;
  export let trafficSort: XrayTrafficSort;
  export let trafficSortAscending: boolean;
  export let pairHeight: number | undefined;
  export let trafficHeight: number | undefined;
  export let pairMinHeight: number;
  export let pairMaxHeight: number;
  export let trafficMinHeight: number;
  export let trafficMaxHeight: number;
  export let onRefresh: () => void;
  export let onSetBalancerTarget: (balancer: XrayBalancerInfo, target: string) => void;
  export let onRestartLogger: () => void;
  export let onTrafficSortChange: (key: XrayTrafficSort) => void;
  export let onPairPointerDown: (event: PointerEvent) => void;
  export let onPairKeyDown: (event: KeyboardEvent) => void;
  export let onTrafficPointerDown: (event: PointerEvent) => void;
  export let onTrafficKeyDown: (event: KeyboardEvent) => void;

  $: trafficTable = sortXrayTrafficRows(
    xrayTrafficRows(snapshot),
    trafficSort,
    trafficSortAscending,
  );
  $: observatoryTable = xrayObservatoryRows(snapshot, $t);
  $: trafficTotal = xrayTrafficTotals(trafficTable);

  function objectOf(value: unknown): Record<string, unknown> | undefined {
    return value && typeof value === 'object' && !Array.isArray(value)
      ? value as Record<string, unknown>
      : undefined;
  }

  function numberOf(value: unknown) {
    return typeof value === 'number' && Number.isFinite(value) ? value : 0;
  }

  function formatBytes(value: number) {
    if (value >= 1024 * 1024 * 1024) return `${(value / 1024 / 1024 / 1024).toFixed(2)} GiB`;
    if (value >= 1024 * 1024) return `${(value / 1024 / 1024).toFixed(2)} MiB`;
    if (value >= 1024) return `${(value / 1024).toFixed(1)} KiB`;
    return `${value} B`;
  }

  function formatXrayUptime(seconds: number) {
    const days = Math.floor(seconds / 86_400);
    const hours = Math.floor((seconds % 86_400) / 3_600);
    const minutes = Math.floor((seconds % 3_600) / 60);
    if (days) return `${days}d ${hours}h`;
    if (hours) return `${hours}h ${minutes}m`;
    if (minutes) return `${minutes}m ${seconds % 60}s`;
    return `${seconds}s`;
  }

  function formatXrayLastSeen(unixSeconds: number) {
    return unixSeconds ? new Date(unixSeconds * 1_000).toLocaleString() : $t('Not reported');
  }

  function xrayUserHasTraffic(user: XrayOnlineUser) {
    return user.uplink > 0 || user.downlink > 0;
  }

  function xrayUserStatusLabel(user: XrayOnlineUser) {
    if (user.online === true) return 'Online';
    if (xrayUserHasTraffic(user)) {
      return user.online === false ? 'Traffic active' : 'Traffic recorded';
    }
    if (user.online === false) return 'Offline';
    return 'Unknown';
  }

  function xrayTrafficRows(value: XrayDashboardSnapshot | null): XrayTrafficRow[] {
    const root = objectOf(value?.metrics);
    const stats = objectOf(root?.stats);
    if (!stats) return [];
    const rows: XrayTrafficRow[] = [];
    for (const scope of ['inbound', 'outbound', 'user']) {
      const entries = objectOf(stats[scope]);
      if (!entries) continue;
      for (const [tag, entry] of Object.entries(entries)) {
        const traffic = objectOf(entry);
        if (!traffic) continue;
        rows.push({
          scope,
          tag,
          uplink: numberOf(traffic.uplink),
          downlink: numberOf(traffic.downlink),
        });
      }
    }
    return rows;
  }

  function sortXrayTrafficRows(
    rows: XrayTrafficRow[],
    key: XrayTrafficSort,
    ascending: boolean,
  ) {
    const direction = ascending ? 1 : -1;
    return [...rows].sort((left, right) => {
      const comparison = key === 'uplink' || key === 'downlink'
        ? left[key] - right[key]
        : left[key].localeCompare(right[key], undefined, {
            numeric: true,
            sensitivity: 'base',
          });
      if (comparison !== 0) return comparison * direction;
      return left.tag.localeCompare(right.tag, undefined, {
        numeric: true,
        sensitivity: 'base',
      });
    });
  }

  function xrayTrafficTotals(rows: XrayTrafficRow[]) {
    const scope = ['outbound', 'inbound', 'user'].find((candidate) =>
      rows.some((row) => row.scope === candidate)
    );
    return rows
      .filter((row) => row.scope === scope)
      .reduce(
        (total, row) => ({
          uplink: total.uplink + row.uplink,
          downlink: total.downlink + row.downlink,
        }),
        { uplink: 0, downlink: 0 },
      );
  }

  function xraySortDirection(key: XrayTrafficSort) {
    if (trafficSort !== key) return 'none';
    return trafficSortAscending ? 'ascending' : 'descending';
  }

  function xrayStrategyName(strategy?: string) {
    switch (strategy) {
      case 'roundRobin': return 'Round robin';
      case 'leastPing': return 'Lowest latency';
      case 'leastLoad': return 'Lowest load';
      case 'random': return 'Random';
      default: return 'Default strategy';
    }
  }

  function xrayStrategyResultLabel(strategy?: string) {
    return strategy === 'leastPing' || strategy === 'leastLoad'
      ? 'Preferred'
      : 'Selection pool';
  }

  function xrayAutomaticTargets(balancer: XrayBalancerInfo) {
    const targets = balancer.principleTargets;
    if (
      balancer.strategy === 'leastPing' ||
      balancer.strategy === 'leastLoad' ||
      balancer.fallbackTarget
    ) {
      return targets.filter((target) => balancer.availableCandidates.includes(target));
    }
    return targets;
  }

  function xrayAutomaticHealthFilteringNeedsFallback(balancer: XrayBalancerInfo) {
    return (
      !balancer.strategy ||
      balancer.strategy === 'random' ||
      balancer.strategy === 'roundRobin'
    ) && !balancer.fallbackTarget;
  }

  function xrayCandidateLabel(candidate: string) {
    const observed = observatoryTable.find((row) => row.tag === candidate);
    return observed?.delay ? `${candidate} · ${observed.delay} ms` : candidate;
  }

  function xrayObservatoryRows(
    value: XrayDashboardSnapshot | null,
    localize: (source: string) => string,
  ) {
    const root = objectOf(value?.metrics);
    const observatory = objectOf(root?.observatory);
    if (!observatory) return [];
    return Object.entries(observatory).map(([name, entry]) => {
      const item = objectOf(entry) ?? {};
      const health = objectOf(item.health_ping ?? item.healthPing);
      const attempts = numberOf(health?.all);
      const failures = numberOf(health?.fail);
      const errorReason = item.last_error_reason ?? item.lastErrorReason;
      const lastError = typeof errorReason === 'string' ? errorReason.trim() : '';
      const lastTry = numberOf(item.last_try_time ?? item.lastTryTime);
      const detail = attempts > 0
        ? `${Math.max(0, attempts - failures)}/${attempts} ${localize('successful probes')}`
        : lastError || (lastTry
          ? `${localize('Checked')} ${new Date(lastTry * 1_000).toLocaleTimeString()}`
          : '');
      return {
        name,
        tag: String(item.outbound_tag ?? item.outboundTag ?? name),
        alive: item.alive === true,
        delay: numberOf(item.delay),
        detail,
        lastError,
      };
    }).sort((left, right) =>
      left.tag.localeCompare(right.tag, undefined, {
        numeric: true,
        sensitivity: 'base',
      })
    );
  }

  function resizeStyle(property: string, height: number | undefined) {
    return height ? `${property}: ${height}px;` : '';
  }

  function visibleTopologyTags(tags: string[] | undefined) {
    return tags?.slice(0, 2) ?? [];
  }

  function hiddenTopologyTagCount(tags: string[] | undefined) {
    return Math.max(0, (tags?.length ?? 0) - 2);
  }
</script>

<div
  id="program-panel-dashboard"
  role="tabpanel"
  tabindex="0"
  aria-labelledby="program-tab-dashboard"
  class="panel xray-dashboard-panel"
>
  <header class="xray-dashboard-header">
    <div class="xray-dashboard-title">
      <span class="xray-dashboard-mark" aria-hidden="true"><Icon name="dashboard" size={20} /></span>
      <div>
        <p class="eyebrow">Xray</p>
        <h2>{$t('Built-in Dashboard')}</h2>
        <p>{$t('Live data from the local Xray API and Metrics endpoints')}</p>
      </div>
    </div>
    <div class="xray-dashboard-actions">
      <span class:online={running} class="xray-live-chip" role="status">
        <i></i>{running ? $t('Live') : $t('Offline')}
      </span>
      <button
        class="xray-refresh-button"
        type="button"
        aria-busy={manualRefreshing}
        on:click={onRefresh}
        disabled={!canRefresh || manualRefreshing}
      >
        <Icon name="restart" size={16} />
        <span>{$t('Refresh')}</span>
      </button>
    </div>
  </header>

  {#if error}
    <ErrorNotice {error} />
  {/if}
  {#if snapshot?.metricsError}
    <div class="generated-config-note warning-note">
      <strong>{$t('Metrics unavailable')}</strong>
      <span>{$t('Check the Xray Metrics port and configuration')}</span>
    </div>
  {/if}

  <div
    style={resizeStyle('--xray-pair-height', pairHeight)}
    class="xray-side-stack"
  >
    <!-- svelte-ignore a11y_no_noninteractive_tabindex (the overflow region must be keyboard-scrollable) -->
    <section
      class="xray-dashboard-block observatory-block"
      tabindex="0"
      aria-labelledby="xray-observatory-title"
    >
      <div class="xray-block-heading">
        <span class="xray-block-icon" aria-hidden="true"><Icon name="activity" size={18} /></span>
        <div>
          <h3 id="xray-observatory-title">{$t('Outbound observatory')}</h3>
          <p>{observatoryTable.length} {$t('Observed outbounds')}</p>
        </div>
      </div>
      {#if observatoryTable.length}
        <div class="xray-observatory-list">
          {#each observatoryTable as row (row.name)}
            <article title={row.lastError || row.tag}>
              <span
                class:running={row.alive}
                class:error-state={!row.alive}
                class="dot"
                aria-hidden="true"
              ></span>
              <div class="xray-observatory-copy">
                <strong>{row.tag}</strong>
                {#if row.detail}<small>{row.detail}</small>{/if}
              </div>
              <code class="xray-observatory-delay">{row.delay ? `${row.delay} ms` : '—'}</code>
            </article>
          {/each}
        </div>
      {:else}
        <div class="xray-empty-state compact">
          {$t('Enable observatory or burst observatory to view outbound health')}
        </div>
      {/if}
    </section>

    <section class="xray-dashboard-block route-control" aria-labelledby="xray-routing-title">
      <div class="xray-block-heading">
        <span class="xray-block-icon" aria-hidden="true"><Icon name="sliders" size={18} /></span>
        <div>
          <h3 id="xray-routing-title">{$t('Routing control')}</h3>
          <p>{$t('Choose an outbound for each balancer')}</p>
        </div>
      </div>
      {#if snapshot?.routingError}
        <div class="xray-routing-message warning">{$t('Routing information is unavailable')}</div>
      {:else if snapshot?.balancers?.length}
        <div class="xray-balancer-list">
          {#each snapshot.balancers as balancer (balancer.tag)}
            <article class="xray-balancer-row">
              <div class="xray-balancer-heading">
                <div>
                  <strong title={balancer.tag}>{balancer.tag}</strong>
                  <small>
                    {$t(xrayStrategyName(balancer.strategy))} ·
                    {balancer.availableCandidates.length}/{balancer.candidates.length}
                    {$t('available')}
                  </small>
                </div>
                <span class:manual={!!balancer.currentTarget}>
                  {$t(balancer.currentTarget ? 'Manual' : 'Automatic')}
                </span>
              </div>
              {#if balancer.error}
                <div class="xray-routing-message warning">
                  {$t('This balancer is not available through the Xray API')}
                </div>
              {/if}
              <label class="xray-balancer-target">
                <span>{$t('Selected outbound')}</span>
                <select
                  class="option-align-start"
                  value={balancer.currentTarget ?? ''}
                  disabled={!!routingBusyTag || !!balancer.error || (!balancer.availableCandidates.length && !balancer.currentTarget)}
                  on:change={(event) => onSetBalancerTarget(balancer, event.currentTarget.value)}
                >
                  <option value="">{$t('Automatic selection')}</option>
                  {#if balancer.currentTarget && !balancer.candidates.includes(balancer.currentTarget)}
                    <option value={balancer.currentTarget} disabled>{balancer.currentTarget}</option>
                  {:else if balancer.currentTarget && !balancer.availableCandidates.includes(balancer.currentTarget)}
                    <option value={balancer.currentTarget} disabled>
                      {balancer.currentTarget} · {$t('Unavailable')}
                    </option>
                  {/if}
                  {#each balancer.availableCandidates as candidate (candidate)}
                    <option value={candidate}>{xrayCandidateLabel(candidate)}</option>
                  {/each}
                </select>
              </label>
              {#if !balancer.currentTarget && xrayAutomaticHealthFilteringNeedsFallback(balancer)}
                <div class="xray-routing-message">
                  {$t('Automatic health filtering requires a fallback outbound for this strategy')}
                </div>
              {/if}
              <div class="xray-balancer-footer">
                <small title={balancer.selectors.join(', ')}>
                  <span>{$t('Matches')}</span>
                  <strong>{balancer.selectors.join(', ') || '—'}</strong>
                </small>
                {#if routingBusyTag === balancer.tag}
                  <span>{$t('Applying')}…</span>
                {:else if xrayAutomaticTargets(balancer).length}
                  <span title={xrayAutomaticTargets(balancer).join(', ')}>
                    {$t(xrayStrategyResultLabel(balancer.strategy))}
                    {xrayAutomaticTargets(balancer).join(', ')}
                  </span>
                {:else if balancer.fallbackTarget}
                  <span class="fallback" title={balancer.fallbackTarget}>
                    {$t('Fallback')} {balancer.fallbackTarget}
                  </span>
                {:else if balancer.availableCandidates.length}
                  <span class="unavailable">
                    {$t(balancer.strategy === 'leastPing' || balancer.strategy === 'leastLoad'
                      ? 'No preferred outbound'
                      : 'Selection pool unavailable')}
                  </span>
                {:else if balancer.candidates.length}
                  <span class="unavailable">{$t('No healthy observed outbound')}</span>
                {:else}
                  <span class="unavailable">{$t('No matching outbound')}</span>
                {/if}
              </div>
            </article>
          {/each}
        </div>
      {:else}
        <div class="xray-empty-state compact">{$t('No routing balancers configured')}</div>
      {/if}
    </section>
    <ResizeSeparator
      label={$t('Resize panel')}
      value={pairHeight ?? 520}
      min={pairMinHeight}
      max={pairMaxHeight}
      onPointerDown={onPairPointerDown}
      onKeyDown={onPairKeyDown}
    />
  </div>

  <section class="xray-dashboard-overview" aria-label={$t('Runtime')}>
    <article class="xray-dashboard-card state">
      <span>{$t('Runtime')}</span>
      <strong>{runtimeStateLabel}</strong>
      <small>{running ? $t('Metrics refresh is active') : $t('Start Xray to read live metrics')}</small>
    </article>
    <article class="xray-dashboard-card numeric">
      <span>{$t('Total uplink')}</span>
      <strong>{formatBytes(trafficTotal.uplink)}</strong>
      <small>{trafficTable.length} {$t('Traffic rows')}</small>
    </article>
    <article class="xray-dashboard-card numeric">
      <span>{$t('Total downlink')}</span>
      <strong>{formatBytes(trafficTotal.downlink)}</strong>
      <small>{observatoryTable.length} {$t('Observed outbounds')}</small>
    </article>
    <article class="xray-dashboard-card endpoint">
      <span>{$t('API endpoint')}</span>
      <code title={dashboard ? `127.0.0.1:${dashboard.apiPort}` : ''}>
        {dashboard ? `127.0.0.1:${dashboard.apiPort}` : '—'}
      </code>
      <small>{$t('Handler, Logger, Stats, Routing and Reflection services')}</small>
    </article>
    <article class="xray-dashboard-card endpoint">
      <span>{$t('Metrics endpoint')}</span>
      <code title={dashboard ? `127.0.0.1:${dashboard.metricsPort}` : ''}>
        {dashboard ? `127.0.0.1:${dashboard.metricsPort}` : '—'}
      </code>
      <small>
        {snapshot?.fetchedUnixMs
          ? new Date(snapshot.fetchedUnixMs).toLocaleTimeString()
          : $t('Not refreshed')}
      </small>
    </article>
  </section>

  <section class="xray-dashboard-block runtime-telemetry-block" aria-labelledby="xray-telemetry-title">
    <div class="xray-block-heading">
      <span class="xray-block-icon" aria-hidden="true"><Icon name="activity" size={18} /></span>
      <div>
        <h3 id="xray-telemetry-title">{$t('Core telemetry')}</h3>
        <p>{$t('Runtime data from StatsService')}</p>
      </div>
    </div>
    {#if snapshot?.systemStats}
      <div class="xray-runtime-telemetry">
        <div><span>{$t('Core uptime')}</span><strong>{formatXrayUptime(snapshot.systemStats.uptimeSeconds)}</strong></div>
        <div><span>{$t('Allocated memory')}</span><strong>{formatBytes(snapshot.systemStats.allocatedBytes)}</strong></div>
        <div><span>{$t('System memory')}</span><strong>{formatBytes(snapshot.systemStats.systemBytes)}</strong></div>
        <div><span>{$t('Goroutines')}</span><strong>{snapshot.systemStats.goroutines.toLocaleString()}</strong></div>
        <div><span>{$t('Live objects')}</span><strong>{snapshot.systemStats.liveObjects.toLocaleString()}</strong></div>
        <div><span>{$t('GC cycles')}</span><strong>{snapshot.systemStats.garbageCollections.toLocaleString()}</strong></div>
      </div>
    {:else}
      <div class="xray-routing-message warning">{$t('Runtime telemetry is unavailable')}</div>
    {/if}
  </section>

  <section class="xray-dashboard-block runtime-api-block" aria-labelledby="xray-runtime-api-title">
    <div class="xray-block-heading">
      <span class="xray-block-icon" aria-hidden="true"><Icon name="grid" size={18} /></span>
      <div>
        <h3 id="xray-runtime-api-title">{$t('Runtime API')}</h3>
        <p>{$t('Topology, user sessions and logger control')}</p>
      </div>
    </div>
    <div class="xray-runtime-api-grid">
      <article>
        <span>{$t('Inbound handlers')}</span>
        <strong>{snapshot?.topology?.inboundTags.length ?? '—'}</strong>
        <div
          class="xray-tag-preview"
          title={snapshot?.topology?.inboundTags.join(', ') ?? ''}
        >
          {#if snapshot?.topologyError}
            <small>{$t('Topology unavailable')}</small>
          {:else if snapshot?.topology?.inboundTags.length}
            {#each visibleTopologyTags(snapshot.topology.inboundTags) as tag (tag)}
              <code>{tag}</code>
            {/each}
            {#if hiddenTopologyTagCount(snapshot.topology.inboundTags)}
              <span>+{hiddenTopologyTagCount(snapshot.topology.inboundTags)}</span>
            {/if}
          {:else}
            <small>{$t('No handlers reported')}</small>
          {/if}
        </div>
      </article>
      <article>
        <span>{$t('Outbound handlers')}</span>
        <strong>{snapshot?.topology?.outboundTags.length ?? '—'}</strong>
        <div
          class="xray-tag-preview"
          title={snapshot?.topology?.outboundTags.join(', ') ?? ''}
        >
          {#if snapshot?.topologyError}
            <small>{$t('Topology unavailable')}</small>
          {:else if snapshot?.topology?.outboundTags.length}
            {#each visibleTopologyTags(snapshot.topology.outboundTags) as tag (tag)}
              <code>{tag}</code>
            {/each}
            {#if hiddenTopologyTagCount(snapshot.topology.outboundTags)}
              <span>+{hiddenTopologyTagCount(snapshot.topology.outboundTags)}</span>
            {/if}
          {:else}
            <small>{$t('No handlers reported')}</small>
          {/if}
        </div>
      </article>
      <article>
        <span>{$t('Online users')}</span>
        {#if snapshot?.onlineUsersError}
          <strong>—</strong>
          <small>{$t('Online totals unavailable')}</small>
        {:else if !snapshot?.onlineUsers?.policyEnabled}
          <strong>—</strong>
          <small>{$t('Enable online statistics in Xray policy')}</small>
        {:else if snapshot.onlineUsers.statusAvailable}
          <strong>{snapshot.onlineUsers.userCount}</strong>
          <small>{snapshot.onlineUsers.addressCount} {$t('active addresses')}</small>
        {:else}
          <strong>—</strong>
          <small>{$t('Online session API unavailable')}</small>
        {/if}
      </article>
      <article class="xray-logger-control">
        <span>{$t('Runtime logger')}</span>
        <small>{$t('Reopens the configured Xray log outputs')}</small>
        <button
          type="button"
          on:click={onRestartLogger}
          disabled={loggerBusy || !running}
        >
          <Icon name="logs" size={17} />
          <span>{$t(loggerBusy ? 'Restarting logger' : 'Restart logger')}{loggerBusy ? '…' : ''}</span>
        </button>
      </article>
    </div>

    {#if snapshot?.onlineUsers}
      <section class="xray-online-users" aria-labelledby="xray-user-statistics-title">
        <header>
          <div>
            <h4 id="xray-user-statistics-title">{$t('User statistics')}</h4>
            <p>{$t('User counters follow Xray policy')}</p>
          </div>
          <small>{snapshot.onlineUsers.userCount} {$t('Online users')}</small>
        </header>
        {#if snapshot.onlineUsers.loopbackOnly}
          <div class="xray-routing-message">
            {$t('Loopback traffic is counted, but not marked online by Xray')}
          </div>
        {:else if snapshot.onlineUsers.policyEnabled && !snapshot.onlineUsers.statusAvailable}
          <div class="xray-routing-message warning">
            {$t('This Xray version does not expose online session details')}
          </div>
        {/if}
        {#if snapshot.onlineUsers.users.length}
          <div class="xray-online-user-list">
            {#each snapshot.onlineUsers.users as user (user.email)}
              <article>
                <header>
                  <div class="xray-user-identity">
                    <span
                      class:online={user.online === true}
                      class:traffic={user.online !== true && xrayUserHasTraffic(user)}
                      class:unknown={user.online === undefined && !xrayUserHasTraffic(user)}
                      aria-hidden="true"
                    ></span>
                    <div>
                      <code title={user.email}>{user.email}</code>
                      <small>{$t(xrayUserStatusLabel(user))}</small>
                    </div>
                  </div>
                  <div class="xray-user-traffic">
                    <span title={$t('Uplink')}>↑ {formatBytes(user.uplink)}</span>
                    <span title={$t('Downlink')}>↓ {formatBytes(user.downlink)}</span>
                  </div>
                </header>
                {#if user.addresses.length}
                  <div class="xray-user-addresses">
                    {#each user.addresses as address (`${user.email}-${address.ip}`)}
                      <div>
                        <code>{address.ip}</code>
                        <small>{$t('Last seen')} {formatXrayLastSeen(address.lastSeenUnix)}</small>
                      </div>
                    {/each}
                  </div>
                {:else}
                  <small>
                    {$t(user.online === true ? 'Address details unavailable' : 'No active addresses')}
                  </small>
                {/if}
              </article>
            {/each}
          </div>
        {:else}
          <div class="xray-empty-state compact">{$t('No user statistics')}</div>
        {/if}
      </section>
    {/if}
  </section>

  <section
    style={resizeStyle('--xray-traffic-height', trafficHeight)}
    class="xray-dashboard-block traffic-block"
    aria-labelledby="xray-traffic-title"
  >
    <div class="xray-block-heading xray-traffic-heading">
      <span class="xray-block-icon" aria-hidden="true"><Icon name="activity" size={18} /></span>
      <div>
        <h3 id="xray-traffic-title">{$t('Traffic statistics')}</h3>
        <p>
          {trafficTable.length} {$t('Traffic rows')} ·
          {$t('User counters follow Xray policy')}
        </p>
      </div>
    </div>
    {#if trafficTable.length}
      <div class="xray-table-scroll">
        <table class="xray-table">
          <colgroup>
            <col class="scope-column" />
            <col class="tag-column" />
            <col class="traffic-column" />
            <col class="traffic-column" />
          </colgroup>
          <thead>
            <tr>
              <th aria-sort={xraySortDirection('scope')}>
                <button type="button" on:click={() => onTrafficSortChange('scope')}>
                  <span>{$t('Scope')}</span>
                  <i
                    class:active={trafficSort === 'scope'}
                    class:ascending={trafficSort === 'scope' && trafficSortAscending}
                  ></i>
                </button>
              </th>
              <th aria-sort={xraySortDirection('tag')}>
                <button type="button" on:click={() => onTrafficSortChange('tag')}>
                  <span>{$t('Tag')}</span>
                  <i
                    class:active={trafficSort === 'tag'}
                    class:ascending={trafficSort === 'tag' && trafficSortAscending}
                  ></i>
                </button>
              </th>
              <th class="number" aria-sort={xraySortDirection('uplink')}>
                <button type="button" on:click={() => onTrafficSortChange('uplink')}>
                  <span>{$t('Uplink')}</span>
                  <i
                    class:active={trafficSort === 'uplink'}
                    class:ascending={trafficSort === 'uplink' && trafficSortAscending}
                  ></i>
                </button>
              </th>
              <th class="number" aria-sort={xraySortDirection('downlink')}>
                <button type="button" on:click={() => onTrafficSortChange('downlink')}>
                  <span>{$t('Downlink')}</span>
                  <i
                    class:active={trafficSort === 'downlink'}
                    class:ascending={trafficSort === 'downlink' && trafficSortAscending}
                  ></i>
                </button>
              </th>
            </tr>
          </thead>
          <tbody>
            {#each trafficTable as row (`${row.scope}-${row.tag}`)}
              <tr>
                <td><span class="scope">{row.scope}</span></td>
                <td class="tag-cell"><span title={row.tag}>{row.tag}</span></td>
                <td class="number"><strong>{formatBytes(row.uplink)}</strong></td>
                <td class="number"><strong>{formatBytes(row.downlink)}</strong></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <div class="xray-empty-state">{$t('No traffic statistics reported yet')}</div>
    {/if}
    <ResizeSeparator
      label={$t('Resize panel')}
      value={trafficHeight ?? 480}
      min={trafficMinHeight}
      max={trafficMaxHeight}
      onPointerDown={onTrafficPointerDown}
      onKeyDown={onTrafficKeyDown}
    />
  </section>
</div>
