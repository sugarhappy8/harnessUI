from users import get_user


def greeting(user_id):
    name = get_user(user_id)
    if name is None:
        return "Hello, stranger"
    return f"Hello, {name}"
