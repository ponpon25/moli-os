# Onboarding : Verisure

1. Il faut une clé maître (`data/master.key`) : le pilote range dans le coffre le jeton, l'appareil
   et le second facteur.
2. De préférence, créer dans Verisure un utilisateur réservé à Moli : se connecter avec le même compte
   que Jeedom ou l'application peut couper leur session.
3. Déclarer `kind = "verisure"` (option `country = "FR"` par défaut, `installation` si le compte en a
   plusieurs) ; `moli-os check-config`, redémarrer. Le tableau de bord dit que l'identifiant manque.
4. L'identifiant et le mot de passe passent chacun par un tube, jamais au clavier ni à l'écran :
   `read -rs V; printf %s "$V" | docker exec -i moli-os /moli-os secrets set <instance> username`
   (puis `password`), puis redémarrer. **Une seule tentative de connexion est faite.**
5. Un nouvel appareil demande un SMS : le tableau de bord le dit. Saisir le code de la même façon
   (`otp_code`), puis `docker restart moli-os`.
6. Si la connexion échoue, Moli s'arrête (`login_locked`) au lieu de réessayer : corriger, lever avec
   `secrets forget <instance> login_locked`, redémarrer.
7. Tester : l'état de l'alarme apparaît ; armer puis désarmer **avec quelqu'un sur place**, le code du
   foyer étant demandé.
