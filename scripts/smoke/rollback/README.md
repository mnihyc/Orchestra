# rollback scenario

After an update, `update.sh rollback` puts the previous release and the database as it was when that release
was left. The scenario writes a person under the new release and shows that: a rollback asks, and does not
guess when nobody can be asked; it says what is lost; afterwards the previous release runs and the person is
gone; and a second rollback goes forward to the newer release with the person back, from the dump of the
database as it stood when the first rollback began.
