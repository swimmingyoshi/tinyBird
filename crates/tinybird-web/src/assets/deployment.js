// Capability discovery changes presentation; the server enforces every boundary.
export async function mountDeployment() {
  try {
    const response = await fetch('/api/deployment');
    if (!response.ok) return;
    const config = await response.json();
    document.documentElement.dataset.deployment = config.mode;
    if (config.mode !== 'local') return;
    const style = document.createElement('style');
    style.textContent = `a[href="/contact"], a[href^="/support/"], #signpost-contact,
      [data-addon-tab="community"], [data-addon-view="community"], #publish-form,
      #account-note, #installed-list, details:has(> #published-list), #moderation,
      [data-popup="lobby-sheet"], #btn-cloud, #btn-vault, #btn-vault-saves, #btn-gallery, #account { display: none !important; }`;
    document.head.append(style);
    const screenshot = document.getElementById('btn-store');
    if (screenshot) screenshot.title = 'Download a picture of the screen';
    const lede = document.querySelector('.addons-lede');
    if (lede?.firstChild?.nodeType === Node.TEXT_NODE) lede.firstChild.textContent = 'Create, import and test live readers in this browser. Export your work as JSON to share it. ';
    if (location.hash === '#community') location.hash = '#manage';
  } catch { /* The page remains usable while the server is unreachable. */ }
}
