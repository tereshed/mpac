const error = new URLSearchParams(location.hash.slice(1)).get('error')
if (error) {
  document.querySelector('#heading').textContent = 'Could not open your workspace'
  document.querySelector('#message').textContent = error
  document.querySelector('#retry').hidden = false
}
