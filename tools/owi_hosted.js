'use strict';
const el = id => document.getElementById(id);
let connectionKey = '';
el('endpoint').value = `${location.origin}/mcp`;
async function request(path, options = {}) {
  const response = await fetch(path, {...options, headers: {
    Authorization: `Bearer ${connectionKey}`, 'Content-Type': 'application/json', ...options.headers
  }});
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || 'The connection could not complete this request.');
  return result;
}
el('connect').onclick = async () => {
  connectionKey = el('key').value.trim();
  el('connection').classList.remove('error');
  el('connection').textContent = 'Connecting…';
  el('connect').disabled = true;
  el('run').disabled = true;
  el('telegram').hidden = true;
  try {
    const status = await request('/api/status');
    el('key').value = '';
    el('connection').textContent = `Connected. ${status.remaining_tasks_today} tasks remaining today. You can continue in your AI tool.`;
    el('run').disabled = false;
    if (status.telegram_url) {
      el('telegram').href = status.telegram_url;
      el('telegram').hidden = false;
    }
  } catch (error) {
    connectionKey = '';
    el('telegram').hidden = true;
    el('connection').classList.add('error');
    el('connection').textContent = error.message;
  } finally { el('connect').disabled = false; }
};
el('copy').onclick = async () => {
  try { await navigator.clipboard.writeText(el('endpoint').value); el('copy').textContent = 'Copied'; }
  catch { el('endpoint').select(); el('copy').textContent = 'Select and copy'; }
};
el('run').onclick = async () => {
  const task = el('task').value.trim();
  if (!task) { el('progress').textContent = 'Describe a task first.'; return; }
  el('run').disabled = true;
  el('progress').textContent = 'Working…';
  el('answer').hidden = true;
  try {
    const result = await request('/api/work', {method: 'POST',
      headers: {'Idempotency-Key': crypto.randomUUID()}, body: JSON.stringify({task})});
    el('answer').textContent = result.output || 'No answer returned.';
    el('answer').hidden = false;
    el('progress').textContent = result.verdict === 'execution_error' ? 'The worker reported an error.' : 'Done. Review the answer before using it.';
  } catch (error) { el('progress').textContent = error.message; }
  finally { el('run').disabled = false; }
};
