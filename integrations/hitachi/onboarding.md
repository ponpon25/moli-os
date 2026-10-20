# Onboarding : climatiseurs Hitachi (airCloud Go)

1. Il faut une clé maître (`data/master.key`) : le pilote lit l'identifiant et le mot de passe dans le coffre.
2. De préférence, inviter dans l'appli airCloud Go un second compte réservé à Moli (Membre du foyer)
   plutôt que de réutiliser le compte du propriétaire.
3. Déclarer `kind = "hitachi"` (options facultatives : `family` si le compte a plusieurs foyers,
   `poll_secs`, 60 par défaut, 30 au plus bas) ; `moli-os check-config`, redémarrer. Le tableau de bord dit
   que l'identifiant manque.
4. L'identifiant (l'e-mail du compte) et le mot de passe passent chacun par un tube, jamais au clavier ni à
   l'écran :
   `read -rs V; printf %s "$V" | docker exec -i moli-os /moli-os secrets set <instance> username`
   (puis `password`), puis redémarrer. **Une seule tentative de connexion par démarrage** ; un refus du
   compte arrête le pilote (`login_locked`) : corriger, `secrets forget <instance> login_locked`, redémarrer.
5. Les clims du compte apparaissent seules (nom donné dans l'appli). Tester sur une clim **qu'on peut
   allumer sans gêner personne**, puis la remettre dans son état d'origine : marche/arrêt, mode, consigne,
   vitesse. L'ordre part au cloud Hitachi, la clim suit en quelques secondes (l'état est relu 3 s puis 10 s
   après).
