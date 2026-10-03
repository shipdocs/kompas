/* Release metadata comes from GitHub, including prereleases used by the preview. */
(async () => {
  const releasesUrl = 'https://github.com/shipdocs/kompas/releases';
  const status = document.getElementById('release-status');
  const safeUrl = value => {
    try {
      const url = new URL(value);
      return url.protocol === 'https:' && url.hostname === 'github.com'
        && url.pathname.startsWith('/shipdocs/kompas/releases/') ? url.href : null;
    } catch { return null; }
  };
  try {
    const response = await fetch('https://api.github.com/repos/shipdocs/kompas/releases?per_page=100', {
      headers: { Accept: 'application/vnd.github+json' },
      signal: AbortSignal.timeout(10000),
      cache: 'no-cache',
    });
    if (!response.ok) throw new Error('Release information unavailable');
    const releases = await response.json();
    if (!Array.isArray(releases)) throw new Error('Invalid release information');
    const release = releases.filter(item => !item.draft && item.published_at
      && item.assets?.some(asset => /amd64\.deb$/.test(asset.name) && safeUrl(asset.browser_download_url)))
      .sort((a, b) => Date.parse(b.published_at) - Date.parse(a.published_at))[0];
    if (!release) throw new Error('No installable release available');
    const assets = release.assets.filter(asset => safeUrl(asset.browser_download_url));
    const deb = assets.find(asset => /amd64\.deb$/.test(asset.name));
    document.querySelectorAll('[data-download]').forEach(link => {
      link.href = safeUrl(deb.browser_download_url);
      link.setAttribute('aria-label', `Download Kompas ${release.tag_name} for amd64`);
    });
    document.querySelectorAll('[data-release]').forEach(link => {
      link.href = safeUrl(release.html_url) || releasesUrl;
    });
    const date = new Intl.DateTimeFormat('en', { dateStyle: 'medium' }).format(new Date(release.published_at));
    const badge = document.getElementById('release-badge');
    if (badge) badge.textContent = `${release.prerelease ? 'Preview' : release.tag_name} · Zorin & Ubuntu`;
    status.textContent = `${release.tag_name} · ${release.prerelease ? 'Preview' : 'Stable'} · ${date} · ${(deb.size / 1048576).toFixed(1)} MB`;
    document.getElementById('install-command').textContent = `sudo apt install ./${deb.name}\nkompas`;
    const list = document.createElement('ul');
    assets.forEach(asset => {
      const item = document.createElement('li');
      const link = document.createElement('a');
      link.href = safeUrl(asset.browser_download_url);
      link.textContent = asset.name;
      item.append(link);
      if (/\.deb$/.test(asset.name)) item.append(` — ${Number(asset.download_count || 0).toLocaleString('en')} downloads`);
      list.append(item);
    });
    document.getElementById('release-assets').replaceChildren(list);
  } catch {
    status.textContent = 'Live release information is unavailable. Open GitHub Releases to choose a package and read the release notes.';
    document.querySelectorAll('[data-download]').forEach(link => {
      link.href = releasesUrl;
      link.textContent = 'View downloads on GitHub ↗';
    });
  }
})();
