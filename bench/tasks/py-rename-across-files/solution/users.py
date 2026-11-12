USERS = {1: "Ada", 2: "Grace"}


def get_user(user_id):
    """The name of a user, or None."""
    return USERS.get(user_id)
