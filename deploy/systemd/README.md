# Systemd units

1. Copy unit files to `/etc/systemd/system/`.
2. Copy `gittree.env` to `/etc/gittree/gittree.env` and adjust values.
3. Reload and start services:

```
sudo systemctl daemon-reload
sudo systemctl enable --now gittree-relay.service
sudo systemctl enable --now gittree-admission.service
sudo systemctl enable --now gittree-state.service
sudo systemctl enable --now gittree-git-http.service
sudo systemctl enable --now gittree-coordinator.service
sudo systemctl enable --now gittree-sync.service
sudo systemctl enable --now gittree-webhook.service
sudo systemctl enable --now gittree-ui.service
```

Check status with `systemctl status <service>`.
